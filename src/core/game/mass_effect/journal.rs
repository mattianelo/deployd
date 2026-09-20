use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};
use crate::utils::paths;

use super::Target;
use super::baseline::{Baseline, directory, originals, relative};
pub(super) use super::operation::Control;
use super::operation::Lease;
use super::package::{SourceFile, validate_payload_name};

pub(super) mod files;
mod generation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    #[serde(default, skip_serializing_if = "super::removal::Removals::is_empty")]
    pub(crate) removals: super::removal::Removals,
    pub(crate) version: u32,
    pub(crate) generation: String,
    pub(crate) profile: String,
    pub(crate) files: Vec<SourceFile>,
    #[serde(default)]
    pub(crate) recipe: Option<super::recipe::Recipe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub(super) size: u64,
    pub(super) sha256: String,
}

impl Identity {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.size <= i64::MAX as u64
                && self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "Invalid MELE deployment file identity"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Operation {
    pub(super) path: String,
    pub(super) before: Option<Identity>,
    pub(super) after: Option<Identity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Journal {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) family: Option<super::family::Change>,
    version: u32,
    pub(crate) id: String,
    pub(crate) game_id: String,
    pub(crate) baseline: String,
    pub(crate) previous: Option<State>,
    pub(crate) desired: State,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    missing_components: Vec<String>,
    operations: Vec<Operation>,
    directories: Vec<String>,
}

pub(crate) struct Deployment {
    pub(crate) removals: super::removal::Removals,
    pub(super) family: Option<super::family::Plan>,
    pub(crate) repair_components: bool,
    pub(crate) previous: Option<String>,
    pub(crate) profile: String,
    pub(crate) source: PathBuf,
    pub(crate) files: Vec<SourceFile>,
    pub(crate) recipe: Option<super::recipe::Recipe>,
}

type Progress = Arc<dyn Fn(usize, usize) + Send + Sync>;

struct Abandoned(Arc<AtomicBool>);
impl Drop for Abandoned {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn target(game: &Game) -> Result<Target> {
    ensure!(
        game.engine == GameEngine::MassEffect && game.data_subdir == "BioGame",
        "Journaled MELE deployment requires a Legendary Edition game"
    );
    [Target::Le1, Target::Le2, Target::Le3]
        .into_iter()
        .find(|target| target.game_id() == game.id)
        .context("Unknown MELE deployment target")
}

pub(super) fn destination(path: &str) -> Result<()> {
    relative(path)?;
    validate_payload_name(path)?;
    let relative = path
        .strip_prefix("BioGame/")
        .context("MELE content deployment cannot modify root files or saves")?;
    ensure!(
        !relative.to_ascii_lowercase().ends_with(".pcsav"),
        "MELE saves are outside game-file deployment"
    );
    if let Some(dlc) = relative.strip_prefix("DLC/") {
        let (dlc, _) = dlc
            .split_once('/')
            .context("MELE DLC deployment requires a file inside a DLC folder")?;
        ensure!(
            dlc.starts_with("DLC_") && dlc.len() > 4,
            "MELE DLC paths require a DLC_* directory"
        );
    } else {
        ensure!(
            !relative
                .split('/')
                .next()
                .unwrap_or_default()
                .eq_ignore_ascii_case("DLC"),
            "MELE DLC paths must use canonical casing"
        );
    }
    Ok(())
}

pub(super) fn state_files(
    state: &State,
    baseline: &Baseline,
    target: Target,
) -> Result<BTreeMap<String, Identity>> {
    ensure!(
        (1..=4).contains(&state.version)
            && Uuid::parse_str(&state.generation).is_ok()
            && !state.profile.is_empty()
            && state.profile.len() <= 256
            && state.files.len() <= 100_000,
        "Unsupported or invalid MELE deployment state"
    );
    if let Some(recipe) = &state.recipe {
        recipe.validate()?;
        ensure!(recipe.target == target, "MELE recipe targets another game");
    }
    let components = super::components::owned(state, baseline, target)?;
    ensure!(
        components.is_empty() || state.version >= 2,
        "Component ownership requires MELE deployment version 2"
    );
    let binaries = super::binary::owned(state)?;
    ensure!(
        binaries.is_empty() || state.version >= 4,
        "Binary ownership requires MELE deployment version 4"
    );
    let mut index = BTreeMap::new();
    ensure!(
        state.removals.is_empty() || state.version >= 3,
        "Intentional removals require MELE deployment version 3"
    );
    state.removals.validate(target)?;
    let mut folded = BTreeSet::new();
    for file in &state.files {
        if !components
            .iter()
            .chain(&binaries)
            .any(|owned| owned == file)
        {
            destination(&file.relative)?;
            validate_official_dlc(&file.relative, baseline, target)?;
        }
        let identity = Identity {
            size: file.size,
            sha256: file.sha256.clone(),
        };
        identity.validate()?;
        ensure!(
            folded.insert(file.relative.to_lowercase()),
            "Duplicate or case-colliding MELE deployment files"
        );
        index.insert(file.relative.clone(), identity);
    }
    for path in &folded {
        let mut parent = Path::new(path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            ensure!(
                !folded.contains(path.to_str().context("Invalid MELE deployment path")?),
                "MELE deployment path is both a file and a directory"
            );
            parent = path.parent();
        }
    }
    ensure!(
        state
            .removals
            .paths
            .iter()
            .all(|path| !folded.contains(&path.to_lowercase())),
        "A MELE file cannot be both installed and removed"
    );
    for file in &baseline.files {
        if super::removal::dlc(&file.relative).is_some_and(|name| {
            state
                .removals
                .dlc
                .iter()
                .any(|scope| scope.eq_ignore_ascii_case(name))
        }) {
            ensure!(
                state.removals.paths.contains(&file.relative) || index.contains_key(&file.relative),
                "Obsolete DLC state must account for every recorded baseline file"
            );
        }
    }
    Ok(index)
}

pub(super) fn validate_official_dlc(path: &str, baseline: &Baseline, target: Target) -> Result<()> {
    if let Some((name, _)) = path
        .strip_prefix("BioGame/DLC/")
        .and_then(|path| path.split_once('/'))
        && target.is_official_dlc(name)
    {
        let prefix = format!("BioGame/DLC/{name}/");
        let index = baseline
            .files
            .partition_point(|file| file.relative < prefix);
        ensure!(
            baseline
                .files
                .get(index)
                .is_some_and(|file| file.relative.starts_with(&prefix)),
            "Official DLC '{name}' is absent from the restoration baseline; repair the game and record a clean baseline before installing this mod"
        );
    }
    Ok(())
}

fn operations(
    baseline: &Baseline,
    previous: Option<&State>,
    desired: &State,
    target: Target,
    missing: &[String],
) -> Result<Vec<Operation>> {
    let original: BTreeMap<_, _> = baseline
        .files
        .iter()
        .map(|file| {
            (
                file.relative.clone(),
                Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
            )
        })
        .collect();
    let owned = previous
        .map(|state| managed_files(state, baseline, target))
        .transpose()?
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for path in missing {
        ensure!(
            seen.insert(path) && owned.iter().any(|file| &file.relative == path),
            "Only previously managed MELE runtime or binary-mod files can be repaired"
        );
    }
    let previous_removed = previous
        .map(|state| state.removals.paths.clone())
        .unwrap_or_default();
    let desired_removed = desired.removals.paths.clone();
    let previous = previous
        .map(|state| state_files(state, baseline, target))
        .transpose()?
        .unwrap_or_default();
    let desired = state_files(desired, baseline, target)?;
    let paths: BTreeSet<_> = previous
        .keys()
        .chain(desired.keys())
        .chain(&previous_removed)
        .chain(&desired_removed)
        .cloned()
        .collect();
    let mut canonical: BTreeMap<_, _> = original
        .keys()
        .map(|path| (path.to_lowercase(), path))
        .collect();
    for path in &paths {
        if let Some(existing) = canonical.insert(path.to_lowercase(), path) {
            ensure!(
                existing == path,
                "MELE deployment paths must preserve baseline and previous deployment casing"
            );
        }
    }
    let mut result = Vec::new();
    for path in paths {
        let before = if missing.contains(&path) || previous_removed.contains(&path) {
            None
        } else {
            previous.get(&path).or_else(|| original.get(&path)).cloned()
        };
        let after = if desired_removed.contains(&path) {
            None
        } else {
            desired.get(&path).or_else(|| original.get(&path)).cloned()
        };
        if before != after {
            result.push(Operation {
                path,
                before,
                after,
            });
        }
    }
    Ok(result)
}

impl Journal {
    fn validate(&self, game: &Game, baseline: &Baseline) -> Result<()> {
        let target = target(game)?;
        ensure!(
            (1..=6).contains(&self.version)
                && self.id == Uuid::parse_str(&self.id)?.to_string()
                && self.game_id == game.id
                && self.baseline == baseline.sha256
                && self.desired.generation == self.id,
            "Unsupported or mismatched MELE deployment journal"
        );
        ensure!(
            self.version >= 2
                || self.missing_components.is_empty()
                    && self.desired.version == 1
                    && self
                        .previous
                        .as_ref()
                        .is_none_or(|state| state.version == 1),
            "Runtime recovery requires MELE journal version 2"
        );
        ensure!(
            self.family.is_none() || self.version >= 3,
            "Shared launcher recovery requires MELE journal version 3"
        );
        ensure!(
            self.version >= 4
                || (self.desired.version < 3
                    && self.previous.as_ref().is_none_or(|state| state.version < 3)),
            "Intentional removal recovery requires MELE journal version 4"
        );
        ensure!(
            self.version >= 5
                || self.desired.version < 4
                    && self.previous.as_ref().is_none_or(|state| state.version < 4),
            "Binary-mod recovery requires MELE journal version 5"
        );
        if let Some(change) = &self.family {
            change.validate(&game.id, &self.desired)?;
        }
        ensure!(
            self.operations
                == operations(
                    baseline,
                    self.previous.as_ref(),
                    &self.desired,
                    target,
                    &self.missing_components
                )?,
            "MELE deployment journal operations do not match its recorded states"
        );
        let mut seen = BTreeSet::new();
        for path in &self.directories {
            relative(path)?;
            ensure!(
                seen.insert(path)
                    && self.operations.iter().any(|entry| entry.after.is_some()
                        && entry.path.starts_with(&format!("{path}/"))),
                "Invalid MELE deployment directory ownership"
            );
        }
        ensure!(
            self.directories.windows(2).all(|pair| pair[0] < pair[1]),
            "Unordered MELE deployment directories"
        );
        Ok(())
    }
}

fn storage(data: &Path, game_id: &str, id: &str) -> PathBuf {
    data.join("mele-transactions").join(game_id).join(id)
}

pub(crate) async fn recover(tracker: Tracker, game: Game) -> Result<()> {
    recover_in(tracker, game, paths::deployd_data_dir()?).await
}

pub(super) async fn recover_in(tracker: Tracker, game: Game, data: PathBuf) -> Result<()> {
    if tracker.mele_journal(&game.id).await?.is_none() {
        return Ok(());
    }
    tokio::spawn(async move {
        let _operation = super::mutation_lock().lock_owned().await;
        let _location = crate::core::location_recovery::activity_lock()
            .try_read_owned()
            .context("Folder access is being changed; retry MELE recovery when it finishes")?;
        tracker.ensure_location_ready(&game.id).await?;
        recover_locked(&tracker, &game, &data).await
    })
    .await
    .context("MELE recovery worker failed")?
}

async fn recover_locked(tracker: &Tracker, game: &Game, data: &Path) -> Result<()> {
    let Some((journal, committed)) = tracker.mele_journal(&game.id).await? else {
        return Ok(());
    };
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("MELE recovery has no restoration baseline")?;
    journal.validate(game, &baseline)?;
    let current = tracker.mele_deployment(&game.id).await?;
    ensure!(
        current
            == if committed {
                Some(journal.desired.clone())
            } else {
                journal.previous.clone()
            },
        "MELE deployment state differs from its recovery journal"
    );
    if committed {
        ensure!(
            tracker
                .mele_recipe(&game.id, &journal.desired.profile)
                .await?
                == journal.desired.recipe,
            "Committed MELE profile recipe differs from its deployment journal"
        );
    }
    let launcher = if let Some(change) = &journal.family {
        let recorded = tracker.mele_family(change.location_id).await?;
        ensure!(
            recorded.as_ref()
                == Some(if committed {
                    &change.desired
                } else {
                    &change.previous
                }),
            "Shared launcher state differs from its recovery journal"
        );
        Some((
            super::family::root(tracker, game, change.location_id).await?,
            change.storage(data, &journal.id),
        ))
    } else {
        None
    };
    let root = storage(data, &game.id, &journal.id);
    let work = journal.clone();
    let game_root = game.path.clone();
    tokio::task::spawn_blocking(move || {
        if !committed {
            files::rollback(&game_root, &root, &work)?;
            if let (Some(change), Some((launcher, stage))) = (&work.family, &launcher) {
                change.rollback(launcher, stage)?;
            }
        } else {
            super::removal::cleanup(&game_root, &baseline, &work.desired, &Control::recovery())?;
        }
        files::cleanup(&root, &work)?;
        if let (Some(change), Some((_, stage))) = (&work.family, &launcher) {
            change.cleanup(stage)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("MELE recovery file worker failed")??;
    tracker.clear_mele_journal(&game.id, &journal.id).await
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Journal publication awaits the package-recipe coordinator"
    )
)]
pub(crate) async fn publish(
    tracker: Tracker,
    game: Game,
    deployment: Deployment,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<State> {
    publish_in(
        tracker,
        game,
        deployment,
        paths::deployd_data_dir()?,
        cancelled,
        progress,
    )
    .await
}

pub(super) async fn publish_in(
    tracker: Tracker,
    game: Game,
    deployment: Deployment,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<State> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(abandoned.clone());
    let control = Control {
        cancelled,
        abandoned,
    };
    publish_with_control(tracker, game, deployment, data, control, progress).await
}

pub(super) async fn publish_with_control(
    tracker: Tracker,
    game: Game,
    deployment: Deployment,
    data: PathBuf,
    control: Control,
    progress: Progress,
) -> Result<State> {
    tokio::spawn(publish_worker(
        tracker, game, deployment, data, control, progress,
    ))
    .await
    .context("MELE deployment worker failed")?
}

async fn publish_worker(
    tracker: Tracker,
    game: Game,
    deployment: Deployment,
    data: PathBuf,
    control: Control,
    progress: Progress,
) -> Result<State> {
    let lease = Lease::acquire(&control).await?;
    publish_with_lease(tracker, game, deployment, data, control, progress, lease).await
}

pub(super) async fn prepare_with_lease(
    tracker: Tracker,
    game: Game,
    deployment: Deployment,
    data: PathBuf,
    control: Control,
    lease: Arc<Lease>,
    shared: bool,
) -> Result<Journal> {
    target(&game)?;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    control.check()?;
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("Finish MELE setup before deploying")?;
    let previous = tracker.mele_deployment(&game.id).await?;
    ensure!(
        previous.as_ref().map(|state| &state.generation) == deployment.previous.as_ref(),
        "MELE deployment changed; rebuild the installation plan"
    );
    let desired = State {
        version: if deployment
            .recipe
            .as_ref()
            .is_some_and(|recipe| recipe.version >= 3)
        {
            4
        } else if !deployment.removals.is_empty() {
            3
        } else if deployment
            .recipe
            .as_ref()
            .is_some_and(|recipe| !recipe.components.is_empty())
        {
            2
        } else {
            1
        },
        removals: deployment.removals,
        generation: Uuid::new_v4().to_string(),
        profile: deployment.profile,
        files: deployment.files,
        recipe: deployment.recipe,
    };
    let family = if shared {
        let family = super::family::inspect(
            &tracker,
            &game,
            &desired,
            deployment.repair_components,
            control.clone(),
        )
        .await?;
        ensure!(
            deployment
                .family
                .as_ref()
                .is_none_or(|expected| family.as_ref() == Some(expected)),
            "Shared launcher files changed after preflight; inspect deployment again"
        );
        family
    } else {
        ensure!(
            deployment.family.is_none(),
            "Shared launcher changes require separate application"
        );
        None
    };
    if let Some(plan) = &family {
        plan.preserve(&tracker, &data, control.clone()).await?;
    }
    let missing_components = if deployment.repair_components {
        let owned = previous
            .as_ref()
            .map(|state| managed_files(state, &baseline, target(&game)?))
            .transpose()?
            .unwrap_or_default();
        let game_root = game.path.clone();
        let control = control.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let mut missing = Vec::new();
            for file in owned {
                control.check()?;
                match std::fs::symlink_metadata(game_root.join(&file.relative)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        files::verify(&game_root, &file.relative, None, &control)?;
                        missing.push(file.relative);
                    }
                    Err(error) => return Err(error).context("Cannot inspect managed MELE runtime"),
                    Ok(_) => {}
                }
            }
            Ok(missing)
        })
        .await??
    } else {
        Vec::new()
    };
    let changes = operations(
        &baseline,
        previous.as_ref(),
        &desired,
        target(&game)?,
        &missing_components,
    )?;
    let originals_needed: Vec<String> = changes
        .iter()
        .filter(|operation| {
            baseline
                .files
                .binary_search_by(|file| file.relative.cmp(&operation.path))
                .is_ok()
        })
        .map(|operation| operation.path.clone())
        .collect();
    let originals = if originals_needed.is_empty() {
        None
    } else {
        Some(
            originals::preserve_with_lease(
                tracker.clone(),
                game.clone(),
                originals_needed,
                data.clone(),
                control.clone(),
                Arc::new(|_, _| {}),
                lease.clone(),
            )
            .await?,
        )
    };
    control.check()?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    ensure!(
        tracker.mele_deployment(&game.id).await? == previous,
        "MELE deployment changed while preserving originals; rebuild the plan"
    );
    let mut journal = Journal {
        family: family.as_ref().map(|plan| plan.change.clone()),
        version: if desired.version >= 4
            || previous.as_ref().is_some_and(|state| state.version >= 4)
        {
            5
        } else if desired.version == 3 || previous.as_ref().is_some_and(|state| state.version == 3)
        {
            4
        } else if family.is_some() {
            3
        } else if desired.version == 2 || previous.as_ref().is_some_and(|state| state.version == 2)
        {
            2
        } else {
            1
        },
        missing_components,
        id: desired.generation.clone(),
        game_id: game.id.clone(),
        baseline: baseline.sha256.clone(),
        previous,
        desired,
        operations: changes,
        directories: Vec::new(),
    };
    let root = storage(&data, &game.id, &journal.id);
    let staged = {
        let data = data.clone();
        let mut journal = journal.clone();
        let root = root.clone();
        let game = game.clone();
        let control = control.clone();
        tokio::task::spawn_blocking(move || {
            directory(&game.path)?;
            journal.directories = files::missing_directories(&game.path, &journal.operations)?;
            journal.validate(&game, &baseline)?;
            super::removal::verify(
                &game.path,
                &baseline,
                journal.previous.as_ref(),
                &super::removal::scopes(journal.previous.as_ref(), &journal.desired),
                &control,
            )?;
            if let Some(plan) = &family {
                plan.stage(&data, &journal.id, &control)?;
            }
            files::stage(
                &game.path,
                &deployment.source,
                &root,
                &journal,
                originals.as_ref().map(|originals| originals.root.as_path()),
                &control,
            )?;
            Ok::<_, anyhow::Error>(journal)
        })
        .await
        .context("MELE deployment staging worker failed")
        .and_then(|result| result)
    };
    match staged {
        Ok(value) => journal = value,
        Err(error) => {
            discard(&root, &journal, &data).await.with_context(|| {
                format!("MELE staging failed ({error:#}); temporary storage needs attention")
            })?;
            return Err(error);
        }
    }
    if let Err(error) = control.check() {
        discard(&root, &journal, &data).await?;
        return Err(error);
    }
    Ok(journal)
}

pub(super) async fn publish_with_lease(
    tracker: Tracker,
    game: Game,
    deployment: Deployment,
    data: PathBuf,
    control: Control,
    progress: Progress,
    lease: Arc<Lease>,
) -> Result<State> {
    recover_locked(&tracker, &game, &data).await?;
    let journal = prepare_with_lease(
        tracker.clone(),
        game.clone(),
        deployment,
        data.clone(),
        control.clone(),
        lease.clone(),
        true,
    )
    .await?;
    let root = storage(&data, &game.id, &journal.id);
    let launcher_root = if let Some(change) = &journal.family {
        Some(super::family::root(&tracker, &game, change.location_id).await?)
    } else {
        None
    };
    let launcher_stage = journal
        .family
        .as_ref()
        .map(|change| change.storage(&data, &journal.id));
    if let Err(error) = tracker.begin_mele_journal(&journal).await {
        if let Some((pending, _)) = tracker.mele_journal(&game.id).await? {
            ensure!(
                pending.id == journal.id,
                "Another MELE journal appeared while staging"
            );
            recover_locked(&tracker, &game, &data).await?;
        } else {
            discard(&root, &journal, &data).await?;
        }
        return Err(error);
    }
    let work = journal.clone();
    let game_root = game.path.clone();
    let applied = {
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("Missing MELE baseline")?;
        let root = root.clone();
        let control = control.clone();
        let progress = progress.clone();
        tokio::task::spawn_blocking(move || {
            if let (Some(change), Some(launcher), Some(stage)) =
                (&work.family, &launcher_root, &launcher_stage)
            {
                change.apply(launcher, stage, &control)?;
            }
            for (index, operation) in work.operations.iter().enumerate() {
                control.check()?;
                files::replace(&game_root, &root, operation, false, &control)?;
                progress(index + 1, work.operations.len() + 1);
            }
            for operation in &work.operations {
                files::verify(
                    &game_root,
                    &operation.path,
                    operation.after.as_ref(),
                    &control,
                )?;
            }
            for file in &work.desired.files {
                files::verify(
                    &game_root,
                    &file.relative,
                    Some(&Identity {
                        size: file.size,
                        sha256: file.sha256.clone(),
                    }),
                    &control,
                )?;
            }
            super::removal::verify(
                &game_root,
                &baseline,
                Some(&work.desired),
                &super::removal::scopes(work.previous.as_ref(), &work.desired),
                &control,
            )?;
            if let (Some(change), Some(launcher)) = (&work.family, &launcher_root) {
                change.verify(launcher, false, &control)?;
            }
            control.check()
        })
        .await
        .context("MELE deployment file worker failed")
        .and_then(|result| result)
    };
    let committed = match applied {
        Ok(()) => match control.check() {
            Ok(()) => tracker.commit_mele_journal(&journal).await,
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    if let Err(error) = committed {
        let Some((stored, was_committed)) = tracker
            .mele_journal(&game.id)
            .await
            .context("Cannot determine MELE deployment outcome; recovery is required")?
        else {
            bail!("MELE deployment journal disappeared; reconciliation is required: {error:#}");
        };
        ensure!(
            stored.id == journal.id,
            "MELE deployment journal was replaced unexpectedly"
        );
        recover_locked(&tracker, &game, &data)
            .await
            .with_context(|| {
                format!(
                    "MELE deployment failed ({error:#}); recovery must finish before continuing"
                )
            })?;
        if !was_committed {
            return Err(error);
        }
    } else {
        recover_locked(&tracker, &game, &data)
            .await
            .context("MELE deployment was committed; cleanup must finish before continuing")?;
    }
    progress(journal.operations.len() + 1, journal.operations.len() + 1);
    Ok(journal.desired)
}

async fn discard(root: &Path, journal: &Journal, data: &Path) -> Result<()> {
    let launcher_stage = journal
        .family
        .as_ref()
        .map(|change| change.storage(data, &journal.id));
    let root = root.to_path_buf();
    let journal = journal.clone();
    tokio::task::spawn_blocking(move || {
        files::cleanup(&root, &journal)?;
        if let (Some(change), Some(stage)) = (&journal.family, &launcher_stage) {
            change.cleanup(stage)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("MELE staging cleanup failed")?
}

#[cfg(test)]
mod tests;

fn managed_files(state: &State, baseline: &Baseline, target: Target) -> Result<Vec<SourceFile>> {
    let mut files = super::components::owned(state, baseline, target)?;
    files.extend(super::binary::owned(state)?);
    Ok(files)
}
