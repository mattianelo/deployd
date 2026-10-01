use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use uuid::Uuid;

use crate::core::tracker::Tracker;
use crate::models::game::Game;
use crate::utils::paths;

use super::Target;
use super::baseline::Baseline;
use super::helper;
use super::journal::{self, Control, Identity};
use super::operation::Lease;
use super::package::{SourceFile, Transformation};

mod dlc;
mod installation;
mod merges;
mod pipeline;
mod preparation;
mod squad_ui;
pub(crate) use preparation::inspect as inspect_prepared_in;
pub(super) use preparation::inspect_sources;
pub(crate) mod sources;

use sources::StoredPackage;

type Progress = Arc<dyn Fn(usize, usize) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Package {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) binary_approval: Option<super::binary::Approval>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) binary_files: Vec<SourceFile>,
    pub(crate) id: String,
    pub(crate) source_sha256: String,
    pub(crate) archive_sha256: Option<String>,
    pub(crate) manifest_version: String,
    pub(crate) mod_version: String,
    pub(crate) enabled: bool,
    pub(crate) options: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipe {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) components: Vec<super::components::Selection>,
    pub(crate) version: u32,
    pub(crate) target: Target,
    pub(crate) backend_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) helper_version: Option<String>,
    pub(crate) language: String,
    pub(crate) packages: Vec<Package>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) launcher: Vec<super::launcher::Entry>,
}

impl Recipe {
    pub(crate) fn enable_runtime_components(&mut self) {
        self.version = self.version.max(2);
        self.components = super::components::required(self.target);
    }
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4).contains(&self.version)
                && self.backend_version == 1
                && self.packages.len() <= 1024,
            "Unsupported MELE recipe or native backend version"
        );
        ensure!(
            [
                "DE", "ES", "FE", "FR", "GE", "IE", "INT", "IT", "JA", "PL", "PLPC", "RA", "RU"
            ]
            .contains(&self.language.as_str()),
            "Unsupported MELE recipe language"
        );
        ensure!(
            self.helper_version.as_deref().is_none_or(|version| version
                == helper::protocol::VERSION
                || matches!(
                    version,
                    "0.7.1" | "0.8.0" | "0.9.0" | "0.9.1" | "0.9.2" | "0.10.0" | "0.11.0"
                )),
            "Unsupported MELE transformation helper version"
        );
        ensure!(
            self.components.is_empty() || self.version >= 2,
            "Runtime components require MELE recipe version 2"
        );
        super::components::validate(&self.components, self.target)?;
        let mut ids = BTreeSet::new();
        for package in &self.packages {
            super::binary::package(package, self.target)?;
            ensure!(
                package.binary_approval.is_none() || self.version >= 3,
                "Binary approvals require MELE recipe version 3"
            );
            ensure!(
                Uuid::parse_str(&package.id)?.to_string() == package.id && ids.insert(&package.id),
                "Invalid or duplicate MELE recipe package identity"
            );
            digest(&package.source_sha256)?;
            if let Some(hash) = &package.archive_sha256 {
                digest(hash)?;
            }
            ensure!(
                (super::manifest::FORMATS.contains(&package.manifest_version.as_str())
                    || package.manifest_version == "manual-1")
                    && !package.mod_version.is_empty()
                    && package.mod_version.len() <= 256
                    && package.options.len() <= 1024
                    && package.options.iter().all(|option| !option.is_empty()
                        && option.len() <= 256
                        && !option.chars().any(char::is_control)),
                "Invalid MELE recipe version or options"
            );
        }
        ensure!(
            self.launcher.is_empty() || self.version >= 4,
            "Launcher components require MELE recipe version 4"
        );
        super::launcher::validate_entries(&self.launcher)?;
        ensure!(
            self.launcher.iter().all(|entry| {
                entry.owner == self.target.game_id()
                    && self.packages.iter().any(|package| package.id == entry.id)
            }),
            "Launcher components must belong to a package in the same game recipe"
        );
        Ok(())
    }
}

fn digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()),
        "Invalid MELE source SHA-256 identity"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedFile {
    pub(crate) package: String,
    pub(crate) source: String,
    pub(crate) destination: SourceFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Collision {
    pub(crate) path: String,
    pub(crate) replaced: String,
    pub(crate) winner: String,
}

pub(crate) struct ValidatedRecipe {
    prepared: Option<preparation::Cached>,
    pub(super) family: Option<super::family::Plan>,
    components: super::components::Plan,
    recipe: Recipe,
    baseline: String,
    packages: BTreeMap<String, StoredPackage>,
    files: Vec<PlannedFile>,
    collisions: Vec<Collision>,
    transformations: BTreeSet<Transformation>,
    merges: merges::Merges,
    installation: installation::Installation,
}

impl ValidatedRecipe {
    pub(crate) fn recipe(&self) -> &Recipe {
        &self.recipe
    }

    pub(super) fn removals(&self) -> &super::removal::Removals {
        &self.installation.removals
    }
    pub(crate) fn component_files(&self) -> &[SourceFile] {
        &self.components.files
    }
    pub(crate) fn files(&self) -> &[PlannedFile] {
        &self.files
    }
    pub(crate) fn collisions(&self) -> &[Collision] {
        &self.collisions
    }
    pub(crate) fn skipped_tlk(&self) -> &[installation::SkippedTlk] {
        &self.installation.skipped
    }
    pub(crate) fn generated_files(&self) -> &BTreeSet<String> {
        &self.merges.generated
    }
    pub(crate) fn transformations(&self) -> &BTreeSet<Transformation> {
        &self.transformations
    }
}

pub(crate) async fn inspect(
    recipe: Recipe,
    baseline: Baseline,
    cancelled: Arc<AtomicBool>,
) -> Result<ValidatedRecipe> {
    inspect_in(recipe, baseline, paths::deployd_data_dir()?, cancelled).await
}

pub(super) async fn inspect_in(
    recipe: Recipe,
    baseline: Baseline,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<ValidatedRecipe> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(abandoned.clone());
    let control = Control::new(cancelled, abandoned);
    let location = crate::core::location_recovery::activity_lock()
        .try_read_owned()
        .context("Folder access is being changed; retry MELE recipe inspection when it finishes")?;
    tokio::task::spawn_blocking(move || {
        let _location = location;
        validate(recipe, baseline, data, &control, None)
    })
    .await
    .context("MELE recipe inspection worker failed")?
}

fn validate(
    mut recipe: Recipe,
    baseline: Baseline,
    data: PathBuf,
    control: &Control,
    preparation: Option<&mut preparation::Session>,
) -> Result<ValidatedRecipe> {
    recipe.validate()?;
    ensure!(
        recipe
            .helper_version
            .as_deref()
            .is_none_or(|version| version == helper::protocol::VERSION),
        "This recipe records an older helper; preview a new deployment with the current Deployd version"
    );
    baseline.validate()?;
    super::merge_dlc::baseline(&baseline)?;
    ensure!(
        baseline.game_id == recipe.target.game_id(),
        "MELE recipe and baseline target different games"
    );
    let mut packages = BTreeMap::new();
    for selection in recipe.packages.iter().filter(|selection| selection.enabled) {
        control.check()?;
        let stored = sources::load(&data, selection, recipe.target, control)?;
        super::alternates::validate_choices(&stored.plan, &selection.options)?;
        packages.insert(selection.id.clone(), stored);
    }
    let installation = installation::Installation::inspect(
        &recipe,
        &mut packages,
        &baseline,
        control,
        preparation,
    )?;
    for selection in &mut recipe.packages {
        selection.binary_files.clear();
        if !selection.enabled {
            continue;
        }
        let stored = packages
            .get(&selection.id)
            .context("Missing binary package source")?;
        for mapping in stored
            .plan
            .files
            .iter()
            .filter(|file| super::binary::root_mapping(&file.destination))
        {
            ensure!(
                selection
                    .binary_approval
                    .as_ref()
                    .is_some_and(|approval| approval.matches(&stored.plan)),
                "Approve this binary mod before deployment"
            );
            let source = stored
                .plan
                .sources
                .iter()
                .find(|file| file.relative == mapping.source)
                .context("Missing binary source")?;
            selection.binary_files.push(SourceFile {
                relative: super::binary::game_path(&mapping.destination)?,
                size: source.size,
                sha256: source.sha256.clone(),
            });
        }
    }
    let dlc = &installation.dlc;
    let mut files: BTreeMap<String, PlannedFile> = BTreeMap::new();
    let mut collisions = Vec::new();
    let mut transformations = BTreeSet::new();
    for selection in recipe.packages.iter().filter(|selection| selection.enabled) {
        let stored = packages
            .get(&selection.id)
            .context("Missing MELE package after recipe inspection")?;
        control.check()?;
        for required in &stored.plan.manifest.required_dlc {
            ensure!(
                super::dependency::Dependency::parse(required, &stored.plan.manifest.format)?
                    .matches_options(dlc, &installation.versions, &installation.options)?,
                "{} requires DLC '{required}'; enable a package meeting this requirement before deploying",
                stored.plan.manifest.name
            );
        }
        for incompatible in &stored.plan.manifest.incompatible_dlc {
            ensure!(
                !super::dependency::Dependency::parse(incompatible, &stored.plan.manifest.format)?
                    .matches_options(dlc, &installation.versions, &installation.options)?,
                "{} is incompatible with DLC '{incompatible}'",
                stored.plan.manifest.name
            );
        }
        transformations.extend(stored.plan.required_transformations());
        let sources: BTreeMap<_, _> = stored
            .plan
            .sources
            .iter()
            .map(|file| (file.relative.as_str(), file))
            .collect();
        for mapping in &stored.plan.files {
            let source = sources
                .get(mapping.source.as_str())
                .context("MELE file plan has an unknown source")?;
            let destination = super::binary::game_path(&mapping.destination)?;
            journal::validate_official_dlc(&destination, &baseline, recipe.target)?;
            let entry = PlannedFile {
                package: selection.id.clone(),
                source: mapping.source.clone(),
                destination: SourceFile {
                    relative: destination.clone(),
                    size: source.size,
                    sha256: source.sha256.clone(),
                },
            };
            if let Some(previous) = files.insert(destination.to_lowercase(), entry) {
                ensure!(
                    previous.destination.relative == destination,
                    "Case-colliding MELE package destinations"
                );
                collisions.push(Collision {
                    path: destination,
                    replaced: previous.package,
                    winner: selection.id.clone(),
                });
            }
        }
        for path in &installation
            .steps
            .iter()
            .find(|step| step.package == selection.id)
            .context("Missing MELE installation step")?
            .removed
        {
            files.remove(&path.to_lowercase());
        }
    }
    let mut paths = BTreeMap::new();
    for path in baseline
        .files
        .iter()
        .map(|file| file.relative.as_str())
        .chain(
            files
                .values()
                .map(|file| file.destination.relative.as_str()),
        )
    {
        let parts: Vec<_> = path.split('/').collect();
        let mut canonical = String::new();
        for (index, part) in parts.iter().enumerate() {
            if !canonical.is_empty() {
                canonical.push('/');
            }
            canonical.push_str(part);
            let value = (canonical.clone(), index + 1 == parts.len());
            if let Some(existing) = paths.insert(canonical.to_lowercase(), value.clone()) {
                ensure!(
                    existing == value,
                    "MELE recipe conflicts with baseline or package path casing or file/directory types"
                );
            }
        }
    }
    control.check()?;
    let files: Vec<_> = files.into_values().collect();
    let mut merges =
        merges::Merges::inspect(&files, &packages, &baseline, &installation.dlc, control)?;
    merges.originals.extend(installation.originals.clone());
    merges
        .generated
        .extend(installation.generated.iter().cloned());
    let texture_runtime = packages
        .values()
        .any(|stored| stored.plan.manifest.texture_runtime)
        || files.iter().any(|file| {
            matches!(
                super::package::transformation(&file.destination.relative),
                Some(Transformation::TextureOverride | Transformation::PrecompiledTextureOverride)
            )
        });
    super::components::texture_runtime(&mut recipe.components, recipe.target, texture_runtime);
    super::components::binary_runtime(
        &mut recipe.components,
        recipe.target,
        files
            .iter()
            .any(|file| super::binary::executable(&file.destination.relative)),
    );
    if !recipe.components.is_empty() {
        recipe.version = recipe.version.max(2);
    }
    recipe.validate()?;
    let components =
        super::components::Plan::inspect(&recipe.components, &baseline, recipe.target)?;
    Ok(ValidatedRecipe {
        prepared: None,
        family: None,
        components,
        recipe,
        baseline: baseline.sha256,
        packages,
        files,
        collisions,
        transformations,
        merges,
        installation,
    })
}

struct Abandoned(Arc<AtomicBool>);
impl Drop for Abandoned {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) struct Destination {
    pub(crate) repair_components: bool,
    pub(crate) backend: Option<helper::Backend>,
    pub(crate) game: Game,
    pub(crate) profile: String,
    pub(crate) previous: Option<String>,
}

pub(crate) async fn deploy(
    tracker: Tracker,
    destination: Destination,
    plan: ValidatedRecipe,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<journal::State> {
    deploy_in(
        tracker,
        destination,
        plan,
        paths::deployd_data_dir()?,
        cancelled,
        progress,
    )
    .await
}

pub(super) async fn deploy_in(
    tracker: Tracker,
    destination: Destination,
    plan: ValidatedRecipe,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<journal::State> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(abandoned.clone());
    let control = Control::new(cancelled, abandoned);
    tokio::spawn(async move {
        let game = destination.game.clone();
        let built = prepare_with_control(
            tracker.clone(),
            destination,
            plan,
            data.clone(),
            control.clone(),
            progress.clone(),
            true,
        )
        .await?;
        let result = journal::publish_with_lease(
            tracker,
            game,
            built.deployment,
            data.clone(),
            control,
            built.progress,
            built.lease,
        )
        .await;
        drop(built.directory);
        if let Ok(state) = &result
            && let Some(recipe) = &state.recipe
        {
            super::generations::finish_result_cache_in(data, recipe.clone(), state.profile.clone())
                .await;
        }
        result
    })
    .await
    .context("MELE recipe deployment worker failed")?
}

pub(super) struct Built {
    pub(super) deployment: journal::Deployment,
    pub(super) directory: TempDir,
    pub(super) lease: Arc<Lease>,
    pub(super) progress: Progress,
    pub(super) required: Vec<SourceFile>,
}

pub(super) async fn prepare_with_control(
    tracker: Tracker,
    destination: Destination,
    mut plan: ValidatedRecipe,
    data: PathBuf,
    control: Control,
    progress: Progress,
    shared: bool,
) -> Result<Built> {
    let Destination {
        repair_components,
        backend,
        game,
        profile,
        previous,
    } = destination;
    let lease = Lease::acquire(&control).await?;
    control.check()?;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    let deployed = tracker.mele_deployment(&game.id).await?;
    ensure!(
        deployed.as_ref().map(|state| &state.generation) == previous.as_ref(),
        "MELE deployment changed; rebuild the installation plan"
    );
    ensure!(
        game.engine == crate::models::game::GameEngine::MassEffect
            && game.id == plan.recipe.target.game_id(),
        "MELE recipe targets another game"
    );
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("MELE has no restoration baseline")?;
    ensure!(
        baseline.sha256 == plan.baseline,
        "MELE baseline changed after recipe inspection"
    );
    ensure!(
        plan.transformations.iter().all(|kind| kind.supported()),
        "This recipe requires transformation coordination before deployment: {}",
        plan.transformations
            .iter()
            .map(|kind| kind.label())
            .collect::<Vec<_>>()
            .join(", ")
    );
    {
        let game = game.clone();
        let deployed = deployed.clone();
        let baseline = baseline.clone();
        let component_plan = super::components::Plan::inspect(
            &plan.recipe.components,
            &baseline,
            plan.recipe.target,
        )?;
        let control = control.clone();
        let target = plan.recipe.target;
        let scopes = deployed
            .as_ref()
            .into_iter()
            .flat_map(|state| &state.removals.dlc)
            .chain(&plan.installation.removals.dlc)
            .cloned()
            .collect();
        tokio::task::spawn_blocking(move || {
            super::removal::verify(&game.path, &baseline, deployed.as_ref(), &scopes, &control)?;
            super::components::preflight(
                &game.path,
                &component_plan,
                deployed.as_ref(),
                &baseline,
                target,
                repair_components,
                &control,
            )
        })
        .await??;
    }
    let family = if shared {
        let family = super::family::inspect_recipe(
            &tracker,
            &game,
            &plan.recipe,
            repair_components,
            control.clone(),
        )
        .await?;
        ensure!(
            plan.family
                .as_ref()
                .is_none_or(|expected| family.as_ref() == Some(expected)),
            "Shared launcher files changed after preview; inspect deployment again"
        );
        family
    } else {
        ensure!(
            plan.family.is_none(),
            "Shared launcher changes require their separate Apply action"
        );
        None
    };
    let required_paths: BTreeSet<_> = plan
        .merges
        .originals
        .keys()
        .chain(plan.installation.originals.keys())
        .map(|path| format!("BioGame/{path}"))
        .chain(
            plan.prepared
                .as_ref()
                .into_iter()
                .flat_map(|cached| cached.inputs.keys().cloned()),
        )
        .chain(
            plan.components
                .needs_original()
                .then(|| super::components::BINK.to_owned()),
        )
        .collect();
    let required = baseline
        .files
        .iter()
        .filter(|file| required_paths.contains(&file.relative))
        .map(|file| SourceFile {
            relative: file.relative.clone(),
            size: file.size,
            sha256: file.sha256.clone(),
        })
        .collect();
    let game_inputs: Vec<_> = plan
        .merges
        .originals
        .values()
        .map(|file| {
            let relative = format!("BioGame/{}", file.path);
            deployed
                .as_ref()
                .and_then(|state| state.files.iter().find(|file| file.relative == relative))
                .cloned()
                .unwrap_or_else(|| SourceFile {
                    relative,
                    size: file.size,
                    sha256: file.sha256.clone(),
                })
        })
        .collect();
    pipeline::verify_game_inputs(game.path.clone(), game_inputs.clone(), control.clone()).await?;
    let condition_inputs = plan
        .prepared
        .as_ref()
        .map(|cached| cached.inputs.clone())
        .unwrap_or_default();
    preparation::verify_inputs(game.path.clone(), condition_inputs.clone(), control.clone())
        .await?;
    let has_pipeline = !plan.merges.generated.is_empty() || plan.installation.has_raw_m3to();
    if has_pipeline {
        ensure!(
            plan.recipe.helper_version.as_deref() == Some(helper::protocol::VERSION),
            "The MELE recipe must pin its transformation helper version before deployment"
        );
        backend
            .as_ref()
            .context("This MELE recipe requires the packaged transformation helper")?;
    }
    let component_originals = if plan.components.needs_original() {
        Some(
            super::baseline::originals::preserve_with_lease(
                tracker.clone(),
                game.clone(),
                vec![super::components::BINK.into()],
                data.clone(),
                control.clone(),
                Arc::new(|_, _| {}),
                lease.clone(),
            )
            .await?,
        )
    } else {
        None
    };
    let component_plan =
        super::components::Plan::inspect(&plan.recipe.components, &baseline, plan.recipe.target)?;
    let runtime = if component_plan.files.is_empty() {
        None
    } else {
        let report = progress.clone();
        Some(
            super::components::prepare(
                component_plan,
                component_originals,
                data.clone(),
                control.clone(),
                Arc::new(move |done, _| report(done, 6000)),
                lease.clone(),
            )
            .await?,
        )
    };
    let progress: Progress = if runtime.is_some() {
        progress(1000, 6000);
        Arc::new(move |done, total| progress(1000 + done * 5000 / total, 6000))
    } else {
        progress
    };
    let originals = if plan.merges.originals.is_empty() {
        None
    } else {
        Some(
            super::baseline::originals::preserve_with_lease(
                tracker.clone(),
                game.clone(),
                plan.merges
                    .originals
                    .keys()
                    .map(|path| format!("BioGame/{path}"))
                    .collect(),
                data.clone(),
                control.clone(),
                Arc::new(|_, _| {}),
                lease.clone(),
            )
            .await?,
        )
    };
    let (prepared, plan) = if let Some(cached) = plan.prepared.take() {
        cached.validate(&game, previous.as_deref())?;
        let control = control.clone();
        tokio::task::spawn_blocking(move || {
            for stored in plan.packages.values() {
                sources::verify(stored, &control)?;
            }
            cached.verify_outputs(&control)?;
            plan.installation.steps_prepared = true;
            Ok::<_, anyhow::Error>((cached.prepared, plan))
        })
        .await
        .context("Prepared MELE output verification failed")??
    } else {
        let control = control.clone();
        let data = data.clone();
        tokio::task::spawn_blocking(move || {
            let prepared = stage(&plan, &data, &control)?;
            Ok::<_, anyhow::Error>((prepared, plan))
        })
        .await
        .context("MELE recipe staging worker failed")??
    };
    let prepared = if has_pipeline {
        pipeline::run(
            pipeline::Work {
                prepared,
                merges: plan.merges,
                originals,
                installation: plan.installation,
                packages: plan.packages,
                backend: backend.context("Missing MELE transformation helper")?,
                game: game.path.clone(),
                data: data.clone(),
            },
            control.clone(),
            progress.clone(),
            lease.clone(),
        )
        .await?
    } else {
        prepared
    };
    let prepared = if let Some(runtime) = runtime {
        let control = control.clone();
        tokio::task::spawn_blocking(move || {
            let mut prepared = prepared;
            runtime.append(prepared.directory.path(), &mut prepared.files, &control)?;
            Ok::<_, anyhow::Error>(prepared)
        })
        .await??
    } else {
        prepared
    };
    let prepared = {
        let control = control.clone();
        tokio::task::spawn_blocking(move || {
            super::m3to::validate_staged(
                &prepared.sources,
                &prepared.files,
                prepared.recipe.target,
                &control,
            )?;
            Ok::<_, anyhow::Error>(prepared)
        })
        .await
        .context("MELE texture validation worker failed")??
    };
    control.check()?;
    pipeline::verify_game_inputs(game.path.clone(), game_inputs, control.clone()).await?;
    let progress: Progress = if has_pipeline {
        Arc::new(move |done, total| progress(4000 + done * 1000 / total, pipeline::PROGRESS_TOTAL))
    } else {
        progress
    };
    preparation::verify_inputs(game.path.clone(), condition_inputs, control.clone()).await?;
    let cached_data = data.clone();
    let cached_recipe = prepared.recipe.clone();
    let cached_keys = prepared.sources.cache_keys();
    tokio::task::spawn_blocking(move || {
        if super::helper::cache::prepared(&cached_data, &cached_recipe, cached_keys).is_err() {
            eprintln!("deployd: transformation cache usage could not be saved; deployment remains available");
        }
    }).await.context("MELE cache bookkeeping worker failed")?;
    Ok(Built {
        deployment: journal::Deployment {
            removals: prepared.removals,
            family,
            repair_components,
            previous,
            profile,
            source: prepared.sources,
            files: prepared.files,
            recipe: Some(prepared.recipe),
        },
        directory: prepared.directory,
        lease,
        progress,
        required,
    })
}

struct Prepared {
    sources: super::candidate::Sources,
    removals: super::removal::Removals,
    directory: TempDir,
    files: Vec<SourceFile>,
    recipe: Recipe,
}

fn stage(plan: &ValidatedRecipe, data: &std::path::Path, control: &Control) -> Result<Prepared> {
    for stored in plan.packages.values() {
        sources::verify(stored, control)?;
    }
    let parent = data.join("mele-rebuilds");
    journal::files::create_directory(&parent)?;
    let directory = tempfile::Builder::new()
        .prefix("recipe-")
        .tempdir_in(parent)?;
    let sources: BTreeMap<_, _> = plan
        .packages
        .iter()
        .map(|(id, stored)| (id.as_str(), stored.root.as_path()))
        .collect();
    let mut candidate: super::candidate::Sources = directory.path().to_path_buf().into();
    let mut files = Vec::new();
    let mappings = if !plan.merges.generated.is_empty() {
        &[][..]
    } else {
        plan.files.as_slice()
    };
    for file in mappings {
        control.check()?;
        let source = sources
            .get(file.package.as_str())
            .context("Missing MELE source package")?;
        candidate.insert(
            file.destination.relative.clone(),
            (*source).to_path_buf(),
            file.source.clone(),
            Identity {
                size: file.destination.size,
                sha256: file.destination.sha256.clone(),
            },
            control,
        )?;
        files.push(file.destination.clone());
    }
    for stored in plan.packages.values() {
        sources::verify(stored, control)?;
    }
    control.check()?;
    Ok(Prepared {
        sources: candidate,
        removals: plan.installation.removals.clone(),
        directory,
        files,
        recipe: plan.recipe.clone(),
    })
}

#[cfg(test)]
mod tests;
