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
    shared, state,
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
}

pub(crate) struct PreparedShared {
    history: History,
    game: Game,
    journal: Journal,
    pub(crate) differences: Vec<String>,
}

impl std::fmt::Debug for PreparedShared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSharedDeployment")
            .field("game", &self.game.id)
            .field("differences", &self.differences)
            .finish_non_exhaustive()
    }
}

impl PreparedShared {
    pub(crate) async fn apply(self, control: Control) -> Result<()> {
        shared::apply(&self.history, &self.game, &self.journal, control).await
    }
}

pub(crate) async fn prepare_shared(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    entries: Vec<crate::core::game::mass_effect::launcher::Entry>,
    control: Control,
) -> Result<PreparedShared> {
    ensure!(
        game.engine == GameEngine::MassEffect,
        "Shared launcher preparation belongs to another engine"
    );
    let history = History::open(tracker, &game.id, cache, true).await?;
    recovery::recover(&history, game).await?;
    let journal = shared::prepare(
        &history,
        game,
        crate::core::game::mass_effect::family::generations::Action::Mods(entries),
        crate::utils::paths::deployd_data_dir()?,
        control,
    )
    .await?;
    let differences = journal
        .changes
        .iter()
        .filter(|change| change.before != change.after)
        .map(|change| format!("Change: {:?}", change.target))
        .collect();
    Ok(PreparedShared {
        history,
        game: game.clone(),
        journal,
        differences,
    })
}

pub(crate) async fn prepare_shared_restore(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    revision: &str,
    control: Control,
) -> Result<PreparedShared> {
    ensure!(
        game.engine == GameEngine::MassEffect,
        "Shared launcher restoration belongs to another engine"
    );
    let history = History::open(tracker, &game.id, cache, false).await?;
    recovery::recover(&history, game).await?;
    let family = tracker
        .folder_location(&game.id, crate::utils::location::FolderRole::Game)
        .await?
        .id
        .to_string();
    let journal = shared::restore(&history, game, &family, revision, control).await?;
    let differences = journal
        .changes
        .iter()
        .filter(|change| change.before != change.after)
        .map(|change| format!("Change: {:?}", change.target))
        .collect();
    Ok(PreparedShared {
        history,
        game: game.clone(),
        journal,
        differences,
    })
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

    pub(crate) fn profile_id(&self) -> Option<&str> {
        self.manifest
            .as_ref()
            .and_then(|manifest| manifest.profile().ok())
            .map(|(id, _)| id)
    }

    pub(crate) fn file_count(&self) -> usize {
        self.files.len()
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
    recovery::recover(&history, game).await?;
    let previous = ownership::initialize(&history).await?;
    let old = match previous.generation.as_deref() {
        Some(id) => Some(history.load_with_control(id, control.clone()).await?),
        None => None,
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
    let mut journal = prepare_files(
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
    Ok(Prepared {
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
    recovery::recover(&history, &game).await?;
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
    Ok(Prepared {
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
) -> Result<Journal> {
    let deployed = history.tracker.get_deployed_files(&game.id).await?;
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
    for file in deployed {
        let target = Target::file(&game.engine, &file.game_rel_original)?;
        if desired_keys.contains(&key(&target)?) {
            continue;
        }
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
    let journal = Journal::prepare(
        history,
        game,
        desired.into_iter().collect(),
        control.clone(),
    )
    .await?;
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
    Ok(journal)
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
    #[tokio::test]
    async fn explicit_activation_retains_complete_sources_and_purge_keeps_history() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) =
            super::super::tests::snapshot_fixture(temp.path()).await?;
        game.engine = GameEngine::Aurora;
        fs::create_dir_all(game.path.join("data"))?;
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
        Ok(())
    }
}
