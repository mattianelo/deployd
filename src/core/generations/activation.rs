mod reuse;
pub(crate) use reuse::prepare_unchanged;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::core::{game, save_manager::SaveSetId, tracker::Tracker};
use crate::models::{
    game::{Game, GameEngine},
    manifest::ModFile,
};

use super::catalog::History;
use super::content::Control;
use super::journal::{Journal, Node};
use super::manifest::{Manifest, Output};
use super::target::Target;
use super::{
    bethesda, coordinator, eclipse, journal, manifest, mele, ownership, prepared, recovery, saves,
    state,
};

pub(crate) struct Prepared {
    history: History,
    game: Game,
    journal: Journal,
    previous: state::State,
    manifest: Option<Manifest>,
    files: Vec<ModFile>,
    saves: SaveSetId,
    pub(crate) differences: Vec<String>,
    outcome: crate::core::deployer::DeployOutcome,
}

#[derive(Debug)]
pub(crate) struct PurgeReport {
    pub(crate) outcome: crate::core::deployer::PurgeOutcome,
    pub(crate) already_purged: bool,
}

impl PurgeReport {
    pub(super) fn from_journal(journal: &Journal) -> Self {
        let mut report = Self {
            already_purged: journal
                .mele
                .as_ref()
                .and_then(|mele| mele.previous.as_ref())
                .is_some_and(|previous| {
                    previous.recipe.is_none()
                        && previous.files.is_empty()
                        && previous.removals.is_empty()
                }),
            outcome: crate::core::deployer::PurgeOutcome {
                files_removed: 0,
                vanilla_files_restored: 0,
                warnings: Vec::new(),
            },
        };
        for change in &journal.changes {
            if change.before == change.after {
                continue;
            }
            match (&change.before, &change.after) {
                (Node::File { .. }, Node::Absent) => report.outcome.files_removed += 1,
                (_, Node::File { .. }) => report.outcome.vanilla_files_restored += 1,
                _ => {}
            }
        }
        report.already_purged &=
            report.outcome.files_removed == 0 && report.outcome.vanilla_files_restored == 0;
        report
    }

    pub(crate) fn message(&self) -> String {
        if self.already_purged {
            "This game's managed deployment is already purged.".into()
        } else if self.outcome.files_removed == 0 && self.outcome.vanilla_files_restored == 0 {
            "Purge complete; no managed file changes were needed.".into()
        } else {
            format!(
                "Purge complete: {} mod file(s) removed, {} original file(s) restored.",
                self.outcome.files_removed, self.outcome.vanilla_files_restored
            )
        }
    }
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedDeployment")
            .field("game", &self.game.id)
            .field("differences", &self.differences)
            .finish_non_exhaustive()
    }
}

impl Prepared {
    pub(crate) fn is_purge(&self) -> bool {
        self.manifest.is_none()
    }

    pub(crate) fn purge_report(&self) -> PurgeReport {
        PurgeReport::from_journal(&self.journal)
    }

    pub(crate) fn profile_id(&self) -> Option<&str> {
        self.manifest
            .as_ref()
            .and_then(|manifest| manifest.profile().ok())
            .map(|(id, _)| id)
    }

    pub(crate) fn take_outcome(&mut self) -> crate::core::deployer::DeployOutcome {
        std::mem::replace(&mut self.outcome, empty_outcome())
    }

    pub(crate) fn save_change_summary(&self) -> Option<&'static str> {
        (self.previous.saves != self.saves).then_some(if self.saves.profile_id().is_some() {
            "Live saves will switch to this profile's isolated bank. The outgoing saves will be preserved."
        } else {
            "Live saves will switch to the shared Global bank. The outgoing profile's saves will be preserved."
        })
    }

    pub(crate) fn change_counts(&self) -> (usize, usize, usize) {
        self.differences
            .iter()
            .fold((0, 0, 0), |(added, removed, changed), difference| {
                if difference.starts_with("Add:") {
                    (added + 1, removed, changed)
                } else if difference.starts_with("Remove:") {
                    (added, removed + 1, changed)
                } else {
                    (added, removed, changed + 1)
                }
            })
    }

    pub(crate) async fn activate(self, control: Control) -> Result<Option<String>> {
        let profile = self
            .manifest
            .as_ref()
            .map(|manifest| manifest.profile().map(|(id, _)| id))
            .transpose()?;
        let deployment = self
            .manifest
            .as_ref()
            .zip(profile)
            .map(|(manifest, profile)| state::Deployment {
                manifest,
                profile,
                files: &self.files,
            });
        coordinator::activate(
            &self.history,
            &self.game,
            &self.journal,
            Some(&self.previous),
            deployment.as_ref(),
            &self.saves,
            control,
        )
        .await?;
        self.manifest.as_ref().map(Manifest::id).transpose()
    }

    pub(crate) async fn discard(self) -> Result<()> {
        journal::discard_save_preparation(&self.history, &self.game, &self.journal.id).await
    }
}

pub(crate) async fn prepare(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    profile: Option<&str>,
    protect_vanilla: bool,
    control: Control,
) -> Result<Prepared> {
    ensure!(
        game.engine != GameEngine::MassEffect,
        "MELE requires its recipe preparation flow"
    );
    tracker.ensure_location_ready(&game.id).await?;
    let history = History::open(tracker, &game.id, cache, true).await?;
    (control.phase)("Recovering interrupted deployment…");
    recovery::recover(&history, game).await?;
    (control.phase)("Preparing deployment files…");
    let previous = ownership::initialize(&history).await?;
    let old = if profile.is_none() && game.engine == GameEngine::Bethesda {
        match previous.generation.as_deref() {
            Some(id) => Some(history.load_with_control(id, control.clone()).await?),
            None => None,
        }
    } else {
        None
    };
    let mut manifest = match profile {
        Some(profile) => Some(
            manifest::capture(
                &history,
                game,
                profile,
                crate::utils::paths::deployd_data_dir()?,
                control.clone(),
            )
            .await?,
        ),
        None => None,
    };
    if let Some(manifest) = &mut manifest {
        manifest.outputs = prepared::files(manifest)?;
    }
    let outputs = manifest
        .as_ref()
        .map(|manifest| manifest.outputs.clone())
        .unwrap_or_default();
    let (mut journal, backed_up) = prepare_files(
        &history,
        game,
        &outputs,
        protect_vanilla && profile.is_some(),
        control.clone(),
    )
    .await?;
    let mut files = match &manifest {
        Some(manifest) => bethesda::preparation::deployment_files(&history, manifest)?,
        None => Vec::new(),
    };
    if game.engine == GameEngine::Bethesda {
        if let Some(input) = manifest.take() {
            let configured = bethesda::preparation::prepare(
                &history,
                game,
                input,
                journal,
                true,
                control.clone(),
            )
            .await?;
            manifest = Some(configured.manifest);
            journal = configured.journal;
            files = configured.files;
        } else if let Some(old) = &old {
            journal =
                bethesda::preparation::purge(&history, game, journal, old, control.clone()).await?;
        }
    }
    if game.engine == GameEngine::Eclipse {
        eclipse::prepare(&history, game, &mut manifest, &mut journal, control.clone()).await?;
    }
    if manifest.is_some() {
        let bindings = files
            .iter()
            .filter(|file| !file.game_rel_original.ends_with('/'))
            .map(|file| {
                let relative = Path::new(&file.cache_path)
                    .strip_prefix(cache)
                    .context("Cache source escapes configured storage")?;
                Ok((
                    Target::file(&game.engine, &file.game_rel_original)?,
                    relative
                        .to_str()
                        .context("Cache source is not UTF-8")?
                        .to_owned(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        journal
            .bind_cache(&history, game, bindings, control.clone())
            .await?;
    }
    let mut target_saves = previous.saves.clone();
    if let Some(manifest) = &manifest
        && game::has_save_management(game)
    {
        (control.phase)("Preparing saves…");
        let prepared = saves::prepare(&history, game, journal, manifest, control.clone()).await?;
        journal = prepared.journal;
        target_saves = prepared.target;
    }
    let differences = journal
        .changes
        .iter()
        .filter(|change| change.before != change.after)
        .map(|change| {
            format!(
                "{}: {:?}",
                match (&change.before, &change.after) {
                    (Node::Absent, _) => "Add",
                    (_, Node::Absent) => "Remove",
                    _ => "Change",
                },
                change.target
            )
        })
        .collect();
    let outcome = deployment_outcome(&game.engine, manifest.as_ref(), &journal, backed_up)?;
    Ok(Prepared {
        outcome,
        history,
        game: game.clone(),
        journal,
        previous,
        manifest,
        files,
        saves: target_saves,
        differences,
    })
}

pub(crate) async fn prepare_mele(
    tracker: &Tracker,
    cache: &Path,
    request: crate::core::game::mass_effect::application::GenerationRequest,
    control: Control,
) -> Result<Prepared> {
    let (destination, recipe, purge) = request.into_parts();
    let game = destination.game.clone();
    let profile = destination.profile.clone();
    ensure!(
        game.engine == GameEngine::MassEffect,
        "MELE generation preparation belongs to another engine"
    );
    tracker.ensure_location_ready(&game.id).await?;
    let history = History::open(tracker, &game.id, cache, true).await?;
    (control.phase)("Recovering interrupted deployment…");
    recovery::recover(&history, &game).await?;
    (control.phase)("Preparing MELE deployment…");
    let previous = ownership::initialize(&history).await?;
    let data = crate::utils::paths::deployd_data_dir()?;
    let (mut manifest, mut journal) = if purge {
        (
            None,
            mele::purge(&history, &game, data, control.clone()).await?,
        )
    } else {
        tracker.save_to_profile(&profile, &game.id).await?;
        let mut manifest =
            manifest::capture(&history, &game, &profile, data.clone(), control.clone()).await?;
        let journal = match mele::reuse(
            &history,
            &game,
            &mut manifest,
            data.clone(),
            control.clone(),
        )
        .await?
        {
            Some(journal) => journal,
            None => {
                mele::prepare(
                    &history,
                    destination,
                    &mut manifest,
                    recipe,
                    data,
                    control.clone(),
                )
                .await?
            }
        };
        (Some(manifest), journal)
    };
    let mut target_saves = previous.saves.clone();
    if let Some(manifest) = &manifest
        && game::has_save_management(&game)
    {
        (control.phase)("Preparing saves…");
        let prepared = saves::prepare(&history, &game, journal, manifest, control.clone()).await?;
        journal = prepared.journal;
        target_saves = prepared.target;
    }
    let differences = journal
        .changes
        .iter()
        .filter(|change| change.before != change.after)
        .map(|change| {
            format!(
                "{}: {:?}",
                match (&change.before, &change.after) {
                    (Node::Absent, _) => "Add",
                    (_, Node::Absent) => "Remove",
                    _ => "Change",
                },
                change.target
            )
        })
        .collect();
    let outcome = deployment_outcome(&game.engine, manifest.as_ref(), &journal, 0)?;
    Ok(Prepared {
        outcome,
        history,
        game,
        journal,
        previous,
        manifest: manifest.take(),
        files: Vec::new(),
        saves: target_saves,
        differences,
    })
}

async fn prepare_files(
    history: &History,
    game: &Game,
    outputs: &[Output],
    protect: bool,
    control: Control,
) -> Result<(Journal, usize)> {
    let backups_before = history
        .tracker
        .get_all_vanilla_backups(&game.id)
        .await?
        .len();
    let deployed = history.tracker.get_deployed_files(&game.id).await?;
    let mut directories = deployed
        .iter()
        .filter(|file| file.game_rel_original.ends_with('/'))
        .map(|file| Target::file(&game.engine, &file.game_rel_original))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut desired: BTreeMap<Target, Node> = outputs
        .iter()
        .map(|output| (output.target.clone(), node(output)))
        .collect();
    let key = |target: &Target| {
        target
            .resolve(game)
            .map(|path| path.to_string_lossy().to_lowercase())
    };
    let desired_keys = desired.keys().map(key).collect::<Result<BTreeSet<_>>>()?;
    let deployed_targets = deployed
        .iter()
        .map(|file| key(&Target::file(&game.engine, &file.game_rel_original)?))
        .collect::<Result<BTreeSet<_>>>()?;
    let mut parents = BTreeSet::new();
    for file in deployed {
        let target = Target::file(&game.engine, &file.game_rel_original)?;
        if desired_keys.contains(&key(&target)?) {
            continue;
        }
        parents.extend(deployment_parents(&target));
        let before = if let Some(backup) = history
            .tracker
            .get_vanilla_backup(&game.id, &file.game_rel_lowercase)
            .await?
        {
            retained_file(history, backup.backup_path, control.clone()).await?
        } else {
            Node::Absent
        };
        desired.insert(target, before);
    }
    for (original, backup) in history.tracker.get_all_vanilla_backups(&game.id).await? {
        let target = Target::file(&game.engine, &original)?;
        if desired_keys.contains(&key(&target)?) || desired.contains_key(&target) {
            continue;
        }
        desired.insert(
            target,
            retained_file(history, backup, control.clone()).await?,
        );
    }
    let directory_game = game.clone();
    let directory_control = control.clone();
    let (desired, directories) = history
        .lease
        .blocking(move || -> Result<_> {
            let mut paths = desired
                .keys()
                .map(|target| {
                    target
                        .resolve(&directory_game)
                        .map(|path| path.to_string_lossy().to_lowercase())
                })
                .collect::<Result<BTreeSet<_>>>()?;
            let data = game::deploy_dir(&directory_game)
                .to_string_lossy()
                .to_lowercase();
            for target in parents {
                directory_control.check()?;
                let path = target.resolve(&directory_game)?;
                let key = path.to_string_lossy().to_lowercase();
                if key == data || !paths.insert(key) {
                    continue;
                }
                match std::fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_dir() => {
                        directories.insert(target.clone());
                        desired.insert(target, Node::Absent);
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("Cannot inspect deployment parent '{}'", path.display())
                        });
                    }
                }
            }
            Ok((desired, directories))
        })
        .await
        .context("Deployment parent inspection worker stopped")??;
    let journal = Journal::prepare(
        history,
        game,
        desired.into_iter().collect(),
        control.clone(),
    )
    .await?;
    let directory_game = game.clone();
    let directory_control = control.clone();
    let journal = history
        .lease
        .blocking(move || {
            preserve_vanilla_directories(journal, &directory_game, &directories, &directory_control)
        })
        .await
        .context("Deployment directory inspection worker stopped")??;
    if protect {
        for output in outputs.iter().filter(|output| output.content.is_some()) {
            if deployed_targets.contains(&key(&output.target)?) {
                continue;
            }
            let change = journal
                .changes
                .iter()
                .find(|change| change.target == output.target)
                .context("Prepared output is missing")?;
            if !matches!(change.before, Node::File { .. }) {
                continue;
            }
            let path = output.target.resolve(game)?;
            let original = output.target.recorded()?;
            crate::core::deployer::backup_vanilla_file(
                game,
                &history.tracker,
                &original.to_lowercase(),
                &original,
                &path,
            )
            .await?;
        }
    }
    let backed_up = history
        .tracker
        .get_all_vanilla_backups(&game.id)
        .await?
        .len()
        .saturating_sub(backups_before);
    Ok((journal, backed_up))
}

fn deployment_parents(target: &Target) -> Vec<Target> {
    let mut target = target.clone();
    let mut parents = Vec::new();
    while let Target::Bethesda { path, .. }
    | Target::Aurora { path, .. }
    | Target::Eclipse { path, .. }
    | Target::Redengine { path, .. }
    | Target::MassEffect { path }
    | Target::MeleLauncher { path } = &mut target
    {
        let Some((parent, _)) = path.rsplit_once('/') else {
            break;
        };
        *path = parent.to_owned();
        parents.push(target.clone());
    }
    parents
}

fn preserve_vanilla_directories(
    mut journal: Journal,
    game: &Game,
    directories: &BTreeSet<Target>,
    control: &Control,
) -> Result<Journal> {
    let mut remaining = journal
        .changes
        .iter()
        .map(|change| Ok((change.target.resolve(game)?, change.after.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut candidates = journal
        .changes
        .iter()
        .enumerate()
        .filter(|(_, change)| {
            directories.contains(&change.target)
                && matches!(change.before, Node::Directory { .. })
                && change.after == Node::Absent
        })
        .map(|(index, change)| Ok((index, change.target.resolve(game)?)))
        .collect::<Result<Vec<_>>>()?;
    candidates.sort_by_key(|(_, path)| std::cmp::Reverse(path.components().count()));
    for (index, path) in candidates {
        control.check()?;
        let mut occupied = remaining.iter().any(|(child, node)| {
            child != &path && child.starts_with(&path) && *node != Node::Absent
        });
        for entry in std::fs::read_dir(&path)
            .with_context(|| format!("Cannot inspect deployed directory '{}'", path.display()))?
        {
            control.check()?;
            let entry = entry.with_context(|| {
                format!(
                    "Cannot inspect an entry in deployed directory '{}'",
                    path.display()
                )
            })?;
            if remaining.get(&entry.path()) != Some(&Node::Absent) {
                occupied = true;
            }
        }
        if occupied {
            let change = &mut journal.changes[index];
            change.after = change.before.clone();
            remaining.insert(path, change.after.clone());
        }
    }
    Ok(journal)
}

fn empty_outcome() -> crate::core::deployer::DeployOutcome {
    crate::core::deployer::DeployOutcome {
        files_total: 0,
        files_added: 0,
        files_removed: 0,
        conflicts_resolved: 0,
        vanilla_files_backed_up: 0,
        vanilla_files_restored: 0,
        warnings: Vec::new(),
    }
}

fn deployment_outcome(
    engine: &GameEngine,
    manifest: Option<&Manifest>,
    journal: &Journal,
    backed_up: usize,
) -> Result<crate::core::deployer::DeployOutcome> {
    let outputs = manifest
        .map(|manifest| manifest.outputs.as_slice())
        .unwrap_or_default();
    let mut outcome = crate::core::deployer::DeployOutcome {
        files_total: outputs
            .iter()
            .filter(|output| output.content.is_some())
            .count(),
        vanilla_files_backed_up: backed_up,
        ..empty_outcome()
    };
    for change in &journal.changes {
        if change.before == change.after {
            continue;
        }
        match &change.after {
            Node::File { .. } => {
                outcome.files_added += 1;
                if !outputs.iter().any(|output| output.target == change.target) {
                    outcome.vanilla_files_restored += 1;
                }
            }
            Node::Absent if matches!(change.before, Node::File { .. }) => {
                outcome.files_removed += 1
            }
            _ => {}
        }
    }
    if let Some(manifest) = manifest {
        use super::records::{Table, text};
        let enabled: BTreeSet<_> = manifest
            .records
            .iter()
            .filter(|rows| rows.table == Table::ProfileMods)
            .flat_map(|rows| &rows.rows)
            .filter(|row| row.get("enabled").and_then(serde_json::Value::as_i64) == Some(1))
            .map(|row| text(row, "mod_id"))
            .collect::<Result<_>>()?;
        let handler = game::engine_handler::handler_for(engine);
        let mut counts = BTreeMap::<String, usize>::new();
        for row in manifest
            .records
            .iter()
            .filter(|rows| rows.table == Table::Files)
            .flat_map(|rows| &rows.rows)
        {
            if enabled.contains(text(row, "mod_id")?) {
                *counts
                    .entry(
                        handler
                            .conflict_key(text(row, "game_rel_lowercase")?)
                            .to_owned(),
                    )
                    .or_default() += 1;
            }
        }
        outcome.conflicts_resolved = counts.values().map(|count| count.saturating_sub(1)).sum();
    }
    Ok(outcome)
}

fn node(output: &Output) -> Node {
    match &output.content {
        Some(identity) => Node::File {
            identity: identity.clone(),
            mode: output.mode,
        },
        None => Node::Directory { mode: output.mode },
    }
}

async fn retained_file(history: &History, path: PathBuf, control: Control) -> Result<Node> {
    let identity = history.retain(path.clone(), control).await?;
    let mode = history
        .lease
        .blocking(move || {
            use std::os::unix::fs::PermissionsExt;
            std::fs::symlink_metadata(path).map(|metadata| metadata.permissions().mode() & 0o777)
        })
        .await??;
    Ok(Node::File { identity, mode })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use super::*;

    // @variants: both
    #[test]
    fn directory_cleanup_preserves_each_engine_anchor() -> Result<()> {
        for (engine, recorded, expected) in [
            (
                GameEngine::Bethesda,
                "Textures/Nested/File.bin",
                vec!["Textures/Nested", "Textures"],
            ),
            (
                GameEngine::Bethesda,
                "../Tools/Nested/File.bin",
                vec!["../Tools/Nested", "../Tools"],
            ),
            (
                GameEngine::Aurora,
                "../system/Mods/File.bin",
                vec!["../system/Mods", "../system"],
            ),
            (
                GameEngine::Aurora,
                "../launcher/Mods/File.bin",
                vec!["../launcher/Mods", "../launcher"],
            ),
            (
                GameEngine::Aurora,
                "../register/Mods/File.bin",
                vec!["../register/Mods", "../register"],
            ),
            (
                GameEngine::Eclipse,
                "~docs~/Mods/Nested/File.bin",
                vec!["~docs~/Mods/Nested", "~docs~/Mods"],
            ),
            (
                GameEngine::REDEngine,
                "Mods/Nested/File.bin",
                vec!["Mods/Nested", "Mods"],
            ),
        ] {
            let target = Target::file(&engine, recorded)?;
            let parents = deployment_parents(&target);
            assert_eq!(
                parents
                    .iter()
                    .map(Target::recorded)
                    .collect::<Result<Vec<_>>>()?,
                expected
            );
            for parent in parents {
                parent.validate(&engine)?;
            }
        }
        assert!(Target::file(&GameEngine::Bethesda, "~docs~/Mods/File.bin").is_err());
        assert!(Target::file(&GameEngine::Eclipse, "../Mods/File.bin").is_err());
        assert!(deployment_parents(&Target::PluginControl { slot: 0 }).is_empty());
        assert!(deployment_parents(&Target::CustomIni { slot: 0 }).is_empty());
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn purge_removes_empty_directories_and_preserves_vanilla_contents() -> Result<()> {
        for prefix in ["", "../", "../Data/"] {
            for (vanilla, explicit) in [(false, false), (false, true), (true, false), (true, true)]
            {
                let temp = tempfile::tempdir()?;
                let (tracker, game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
                fs::create_dir_all(game.data_dir())?;
                let directory = Target::file(&game.engine, &format!("{prefix}Textures/Nested/"))?;
                let nested = directory.resolve(&game)?;
                fs::create_dir_all(&nested)?;
                fs::write(nested.join("Managed.bin"), b"managed")?;
                if vanilla {
                    fs::write(nested.join("Vanilla.bin"), b"vanilla")?;
                }
                let files = [
                    "Textures/",
                    "Textures/Nested/",
                    "Textures/Nested/Managed.bin",
                ]
                .into_iter()
                .filter(|relative| explicit || !relative.ends_with('/'))
                .map(|relative| {
                    let original = format!("{prefix}{relative}");
                    ModFile {
                        mod_id: "winner".into(),
                        game_rel_lowercase: original.to_lowercase(),
                        game_rel_original: original,
                        cache_path: temp.path().join("winner/file.txt").display().to_string(),
                    }
                })
                .collect::<Vec<_>>();
                tracker.record_deployed_files(&game.id, &files).await?;
                let history = History::open(&tracker, &game.id, temp.path(), true).await?;
                let (journal, _) =
                    prepare_files(&history, &game, &[], false, Control::default()).await?;
                journal
                    .persist(&history, &game, "purge", BTreeMap::new())
                    .await?;
                journal.apply(&history, &game, Control::default()).await?;
                assert!(!nested.join("Managed.bin").exists());
                assert!(game.path.is_dir());
                assert!(game.data_dir().is_dir());
                assert_eq!(nested.exists(), vanilla);
                assert_eq!(nested.parent().context("Missing parent")?.exists(), vanilla);
                if vanilla {
                    assert_eq!(fs::read(nested.join("Vanilla.bin"))?, b"vanilla");
                }
                journal.recover(&history, &game, false).await?;
                assert_eq!(fs::read(nested.join("Managed.bin"))?, b"managed");
                if vanilla {
                    assert_eq!(fs::read(nested.join("Vanilla.bin"))?, b"vanilla");
                }
            }
        }
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn purge_keeps_directories_needed_by_restored_originals() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
        let directory = game.data_dir().join("Textures");
        fs::create_dir_all(&directory)?;
        let original = temp.path().join("original.bin");
        fs::write(&original, b"original")?;
        tracker
            .save_vanilla_backup(
                &game.id,
                "textures/original.bin",
                "Textures/Original.bin",
                &original,
            )
            .await?;
        tracker
            .record_deployed_files(
                &game.id,
                &[ModFile {
                    mod_id: "winner".into(),
                    game_rel_lowercase: "textures/".into(),
                    game_rel_original: "Textures/".into(),
                    cache_path: temp.path().join("winner").display().to_string(),
                }],
            )
            .await?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let (journal, _) = prepare_files(&history, &game, &[], false, Control::default()).await?;
        journal
            .persist(&history, &game, "purge", BTreeMap::new())
            .await?;
        journal.apply(&history, &game, Control::default()).await?;
        assert_eq!(fs::read(directory.join("Original.bin"))?, b"original");
        journal.recover(&history, &game, false).await?;
        assert!(directory.is_dir());
        assert!(!directory.join("Original.bin").exists());
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn purge_preserves_untracked_symlinks_and_rejects_new_files_after_preparation()
    -> Result<()> {
        for added_later in [false, true] {
            let temp = tempfile::tempdir()?;
            let (tracker, game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
            let directory = game.data_dir().join("Textures");
            fs::create_dir_all(&directory)?;
            let vanilla = directory.join("Vanilla.bin");
            if !added_later {
                std::os::unix::fs::symlink(temp.path().join("missing"), &vanilla)?;
            }
            tracker
                .record_deployed_files(
                    &game.id,
                    &[ModFile {
                        mod_id: "winner".into(),
                        game_rel_lowercase: "textures/".into(),
                        game_rel_original: "Textures/".into(),
                        cache_path: temp.path().join("winner").display().to_string(),
                    }],
                )
                .await?;
            let history = History::open(&tracker, &game.id, temp.path(), true).await?;
            let (journal, _) =
                prepare_files(&history, &game, &[], false, Control::default()).await?;
            journal
                .persist(&history, &game, "purge", BTreeMap::new())
                .await?;
            if added_later {
                fs::write(&vanilla, b"vanilla")?;
            }
            assert_eq!(
                journal
                    .apply(&history, &game, Control::default())
                    .await
                    .is_err(),
                added_later
            );
            assert!(fs::symlink_metadata(&vanilla).is_ok());
            journal.recover(&history, &game, false).await?;
            assert!(fs::symlink_metadata(&vanilla).is_ok());
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn purge_report_counts_restored_files_without_counting_directories() -> Result<()> {
        use serde_json::json;
        let file = |hash: char| json!({"File": {"identity": {"size": 1, "sha256": hash.to_string().repeat(64)}, "mode": 420}});
        let change = |path: &str, before: serde_json::Value, after: serde_json::Value| {
            json!({
                "target": {"MassEffect": {"path": path}}, "before": before, "after": after
            })
        };
        let journal: Journal = serde_json::from_value(json!({
            "version": 1, "id": uuid::Uuid::new_v4().to_string(), "game": "mass-effect-le1",
            "changes": [
                change("Engine.pcc", file('a'), file('b')),
                change("MissingOriginal.pcc", json!("Absent"), file('c')),
                change("Mod.pcc", file('d'), json!("Absent")),
                change("Untouched.pcc", file('e'), file('e')),
                change("EmptyDirectory", json!({"Directory": {"mode": 493}}), json!("Absent"))
            ]
        }))?;
        let report = PurgeReport::from_journal(&journal);
        assert_eq!(report.outcome.files_removed, 1);
        assert_eq!(report.outcome.vanilla_files_restored, 2);
        assert!(!report.already_purged);
        assert!(report.message().contains("2 original file(s) restored"));
        let mut unchanged = journal;
        unchanged.changes.clear();
        let report = PurgeReport::from_journal(&unchanged);
        assert!(!report.message().contains("clean"));
        assert!(!report.already_purged);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn explicit_activation_retains_complete_sources_and_purge_keeps_history() -> Result<()> {
        for keep_vanilla in [false, true] {
            let temp = tempfile::tempdir()?;
            let (tracker, mut game, profile) =
                super::super::tests::snapshot_fixture(temp.path()).await?;
            game.engine = GameEngine::Aurora;
            fs::create_dir_all(game.path.join("data/Unrelated"))?;
            let cached_directory = temp.path().join("winner/Nested");
            fs::create_dir(&cached_directory)?;
            fs::create_dir(cached_directory.join("Inner"))?;
            let cached_file = cached_directory.join("Inner/Managed.bin");
            fs::write(&cached_file, b"managed")?;
            sqlx::query("INSERT INTO mod_files(mod_id,game_rel_lowercase,game_rel_original,cache_path) VALUES ('winner','nested/inner/managed.bin','Nested/Inner/Managed.bin',?)")
                .bind(cached_file.to_string_lossy().as_ref())
                .execute(&tracker.pool)
                .await?;
            tracker.switch_profile(&game.id, &profile).await?;
            tracker.save_to_profile(&profile, &game.id).await?;
            let prepared = prepare(
                &tracker,
                &game,
                temp.path(),
                Some(&profile),
                false,
                Control::default(),
            )
            .await?;
            assert!(!game.path.join("data/File.txt").exists());
            let id = prepared
                .activate(Control::default())
                .await?
                .context("Generation was not published")?;
            let live = game.path.join("data/File.txt");
            let vanilla = game.path.join("data/Nested/Vanilla.bin");
            if keep_vanilla {
                fs::write(&vanilla, b"vanilla")?;
            }
            assert_eq!(fs::read(&live)?, b"winner");
            assert_eq!(
                fs::metadata(&live)?.ino(),
                fs::metadata(temp.path().join("winner/file.txt"))?.ino()
            );
            let purge = prepare(
                &tracker,
                &game,
                temp.path(),
                None,
                false,
                Control::default(),
            )
            .await?;
            purge.activate(Control::default()).await?;
            assert!(!live.exists());
            assert!(game.path.join("data/Unrelated").is_dir());
            if keep_vanilla {
                assert_eq!(fs::read(&vanilla)?, b"vanilla");
            } else {
                assert!(!game.path.join("data/Nested").exists());
            }
            assert!(!game.path.join("data/Nested/Inner").exists());
            assert!(tracker.get_deployed_files(&game.id).await?.is_empty());
            let history = History::open(&tracker, &game.id, temp.path(), false).await?;
            let retained = history.load(&id).await?;
            assert!(
                retained
                    .sources
                    .iter()
                    .any(|source| source.path == "cache/disabled/file.txt")
            );
            assert!(
                retained
                    .sources
                    .iter()
                    .any(|source| source.path == "cache/loser/file.txt")
            );
        }
        Ok(())
    }
}
