use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result, ensure};

use crate::core::{deployer::DeployOutcome, tracker::Tracker};
use crate::models::game::Game;

use super::{
    components,
    helper::Backend,
    journal, library,
    operation::Control,
    recipe::{self, Recipe, ValidatedRecipe},
};

pub(crate) struct Preview {
    pub(crate) game: Game,
    pub(crate) profile: String,
    pub(crate) recipe: Recipe,
    pub(crate) purge: bool,
    pub(crate) repair: bool,
    previous: Option<journal::State>,
    plan: ValidatedRecipe,
    backend: Option<Backend>,
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MelePreview")
            .field("game", &self.game.id)
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

pub(crate) struct Request {
    pub(crate) game: Game,
    pub(crate) profile: String,
    pub(crate) language: String,
    pub(crate) purge: bool,
    pub(crate) repair: bool,
}

pub(crate) async fn deploy(
    tracker: Tracker,
    request: Request,
    cancelled: Arc<AtomicBool>,
    progress: Arc<dyn Fn(usize, usize) + Send + Sync>,
) -> Result<DeployOutcome> {
    deploy_in(
        tracker,
        request,
        crate::utils::paths::deployd_data_dir()?,
        cancelled,
        progress,
    )
    .await
}

pub(super) async fn deploy_in(
    tracker: Tracker,
    request: Request,
    data: std::path::PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Arc<dyn Fn(usize, usize) + Send + Sync>,
) -> Result<DeployOutcome> {
    let preview = preview_with_progress(
        tracker.clone(),
        request,
        data.clone(),
        cancelled.clone(),
        progress.clone(),
    )
    .await?;
    apply_in(tracker, preview, data, cancelled, progress).await
}

#[cfg(test)]
pub(super) async fn preview_in(
    tracker: Tracker,
    request: Request,
    data: std::path::PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<Preview> {
    preview_with_progress(tracker, request, data, cancelled, Arc::new(|_, _| {})).await
}

async fn preview_with_progress(
    tracker: Tracker,
    request: Request,
    data: std::path::PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Arc<dyn Fn(usize, usize) + Send + Sync>,
) -> Result<Preview> {
    let Request {
        game,
        profile,
        language,
        purge,
        repair,
    } = request;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("MELE restoration baseline is unavailable; reopen this game to finish setup")?;
    let previous = tracker.mele_deployment(&game.id).await?;
    let recipe = library::desired(&tracker, &game, language, purge).await?;
    let mut plan = recipe::inspect_prepared_in(
        tracker.clone(),
        recipe::Destination {
            game: game.clone(),
            profile: profile.clone(),
            previous: previous.as_ref().map(|state| state.generation.clone()),
            repair_components: repair,
            backend: None,
        },
        recipe.clone(),
        data,
        cancelled.clone(),
        progress,
    )
    .await?;
    plan.family = super::family::inspect_recipe(
        &tracker,
        &game,
        plan.recipe(),
        repair,
        Control::new(cancelled.clone(), Arc::new(AtomicBool::new(false))),
    )
    .await?;
    let component_plan =
        components::Plan::inspect(&plan.recipe().components, &baseline, recipe.target)?;
    let runtime_root = game.path.clone();
    let installed = previous.clone();
    let target = recipe.target;
    let requires_helper = plan
        .transformations()
        .iter()
        .any(|kind| *kind != super::package::Transformation::PrecompiledTextureOverride);
    let removal_scopes = previous
        .as_ref()
        .into_iter()
        .flat_map(|state| &state.removals.dlc)
        .chain(&plan.removals().dlc)
        .cloned()
        .collect();
    let backend = tokio::task::spawn_blocking(move || {
        let control = Control::new(cancelled, Arc::new(AtomicBool::new(false)));
        super::removal::verify(
            &runtime_root,
            &baseline,
            installed.as_ref(),
            &removal_scopes,
            &control,
        )?;
        components::preflight(
            &runtime_root,
            &component_plan,
            installed.as_ref(),
            &baseline,
            target,
            repair,
            &control,
        )?;
        if requires_helper {
            Backend::packaged().map(Some)
        } else {
            Ok(None)
        }
    })
    .await??;
    Ok(Preview {
        game,
        profile,
        recipe,
        purge,
        repair,
        previous,
        plan,
        backend,
    })
}

pub(super) async fn apply_in(
    tracker: Tracker,
    preview: Preview,
    data: std::path::PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Arc<dyn Fn(usize, usize) + Send + Sync>,
) -> Result<DeployOutcome> {
    let current = library::desired(
        &tracker,
        &preview.game,
        preview.recipe.language.clone(),
        preview.purge,
    )
    .await?;
    ensure!(
        current == preview.recipe,
        "The MELE mod list changed after preview; inspect deployment again"
    );
    tracker
        .save_to_profile(&preview.profile, &preview.game.id)
        .await?;
    let old = preview
        .previous
        .as_ref()
        .map(|state| state.files.as_slice())
        .unwrap_or_default()
        .to_vec();
    let conflicts = preview.plan.collisions().len();
    let old_removed = preview
        .previous
        .as_ref()
        .map(|state| state.removals.paths.clone())
        .unwrap_or_default();
    let warnings = preview
        .plan
        .skipped_tlk()
        .iter()
        .map(|skipped| format!("An optional TLK target was unavailable: {}", skipped.target))
        .collect::<Vec<_>>();
    let state = recipe::deploy_in(
        tracker,
        recipe::Destination {
            game: preview.game,
            profile: preview.profile,
            previous: preview.previous.map(|state| state.generation),
            repair_components: preview.repair,
            backend: preview.backend,
        },
        preview.plan,
        data,
        cancelled,
        progress,
    )
    .await?;
    Ok(DeployOutcome {
        files_total: state.files.len(),
        files_added: state
            .files
            .iter()
            .filter(|file| !old.contains(file))
            .count(),
        files_removed: old
            .iter()
            .filter(|file| !state.files.iter().any(|new| new.relative == file.relative))
            .map(|file| &file.relative)
            .chain(
                state
                    .removals
                    .paths
                    .iter()
                    .filter(|path| !old_removed.contains(*path)),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        conflicts_resolved: conflicts,
        vanilla_files_backed_up: 0,
        vanilla_files_restored: 0,
        warnings,
    })
}
