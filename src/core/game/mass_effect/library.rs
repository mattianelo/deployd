use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::{installer::AddResult, tracker::Tracker};
use crate::models::{
    download::NexusIds,
    game::{Game, GameEngine},
    manifest::ModFile,
    mod_entry::{InstallTarget, ModEntry},
};
use crate::utils::paths;

use super::{
    Target,
    operation::{Control, Lease},
    package::PackagePlan,
    recipe::{self, Package, Recipe},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    version: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) writable_cache: bool,
    pub(crate) target: Target,
    pub(crate) package: Package,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launcher: Option<super::launcher::Entry>,
}

impl Record {
    pub(crate) fn bind_writable_cache(&mut self) {
        self.version = self.version.max(3);
        self.writable_cache = true;
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4).contains(&self.version)
                && (!self.writable_cache || self.version >= 3)
                && (self.package.binary_approval.is_none() || self.version >= 2)
                && (self.launcher.is_none() || self.version >= 4),
            "Unsupported MELE library record version"
        );
        recipe(
            self.target,
            vec![self.package.clone()],
            self.launcher.clone().into_iter().collect(),
            "INT".into(),
            false,
        )
        .validate()
    }
}

pub(crate) fn target(game: &Game) -> Result<Target> {
    ensure!(
        game.engine == GameEngine::MassEffect && game.data_subdir == "BioGame",
        "Select a Legendary Edition game"
    );
    [Target::Le1, Target::Le2, Target::Le3]
        .into_iter()
        .find(|target| target.game_id() == game.id)
        .context("Unknown MELE game")
}

pub(crate) struct Import {
    pub(crate) binary_approved: bool,
    pub(crate) source: PathBuf,
    pub(crate) plan: PackagePlan,
    pub(crate) game: Game,
    pub(crate) name: String,
    pub(crate) options: BTreeSet<String>,
    pub(crate) nexus: Option<NexusIds>,
    pub(crate) archive_hash: Option<String>,
    pub(crate) archive_path: Option<String>,
    pub(crate) replace: Option<String>,
    pub(crate) launcher: Option<super::launcher::Bundled>,
}

type Progress = Arc<dyn Fn(usize, usize) + Send + Sync>;

pub(crate) async fn import(
    tracker: Tracker,
    request: Import,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<AddResult> {
    import_in(
        tracker,
        request,
        paths::deployd_data_dir()?,
        cancelled,
        progress,
    )
    .await
}

async fn import_in(
    tracker: Tracker,
    request: Import,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<AddResult> {
    let target = target(&request.game)?;
    ensure!(
        target == request.plan.manifest.target,
        "The archive targets another game"
    );
    ensure!(
        !request.name.trim().is_empty() && request.name.len() <= 1024,
        "Enter a valid mod name"
    );
    ensure!(
        request
            .plan
            .required_transformations()
            .iter()
            .all(|kind| kind.supported()),
        "This MELE package requires a format that is not supported yet"
    );
    super::alternates::validate_choices(&request.plan, &request.options)?;
    let previous_approval = if let Some(id) = &request.replace {
        tracker
            .mele_package(id)
            .await?
            .and_then(|record| record.package.binary_approval)
    } else {
        None
    };
    let binary_approval = if request.plan.needs_binary_approval() {
        ensure!(
            request.binary_approved
                || previous_approval
                    .as_ref()
                    .is_some_and(|approval| approval.matches(&request.plan)),
            "Approve this mod's plugins before adding it to the library"
        );
        Some(super::binary::Approval::for_plan(&request.plan))
    } else {
        None
    };
    tracker.ensure_location_ready(&request.game.id).await?;
    tracker.ensure_no_mele_journal(&request.game.id).await?;
    let import_root = request.source.clone();
    let stored = recipe::sources::retain_in(
        request.source,
        request.plan.clone(),
        request.archive_hash.clone(),
        data.clone(),
        cancelled.clone(),
        progress,
    )
    .await?;
    let control = Control::new(cancelled, Arc::new(AtomicBool::new(false)));
    let _lease = Lease::acquire(&control).await?;
    tracker.ensure_location_ready(&request.game.id).await?;
    tracker.ensure_no_mele_journal(&request.game.id).await?;
    let previous = if let Some(id) = &request.replace {
        Some(
            tracker
                .list_mods(&request.game.id)
                .await?
                .into_iter()
                .find(|entry| &entry.id == id)
                .context("The mod to replace no longer exists")?,
        )
    } else {
        None
    };
    let mut package = stored.selection();
    if let Some(previous) = &previous {
        package.id = previous.id.clone();
    }
    package.options = request.options;
    package.binary_approval = binary_approval;
    let launcher = request
        .launcher
        .map(|mut bundled| -> Result<_> {
            ensure!(
                !bundled.source.is_absolute()
                    && bundled
                        .source
                        .components()
                        .all(|component| matches!(component, std::path::Component::Normal(_))),
                "Launcher package source escapes the inspected archive"
            );
            bundled.entry.id = package.id.clone();
            bundled.entry.owner = request.game.id.clone();
            bundled.entry.approval = bundled.entry.source_sha256.clone();
            Ok((bundled.entry, import_root.join(bundled.source)))
        })
        .transpose()?;
    let priority = match &previous {
        Some(entry) => entry.priority,
        None => tracker.next_priority(&request.game.id).await?,
    };
    let entry = ModEntry {
        id: package.id.clone(),
        game_id: request.game.id,
        name: request.name,
        archive_hash: request.archive_hash,
        archive_path: request.archive_path,
        installed_at: Some(chrono::Utc::now().to_rfc3339()),
        enabled: previous.as_ref().is_none_or(|entry| entry.enabled),
        priority,
        nexus_mod_id: request.nexus.as_ref().map(|ids| ids.mod_id),
        nexus_file_id: request.nexus.as_ref().map(|ids| ids.file_id),
        nexus_domain: request.nexus.map(|ids| ids.domain),
        version: Some(request.plan.manifest.version.clone()),
        author: Some(request.plan.manifest.author.clone()),
        nexus_description: None,
        latest_version: None,
        nexus_file_name: None,
        nexus_is_primary: false,
        archive_md5: None,
        install_target: InstallTarget::Data,
        notes: None,
    };
    let files = tracked_files(&entry.id, &stored, &package.options)?;
    if let Some((launcher, source)) = &launcher {
        let data = data.clone();
        let source = source.clone();
        let launcher = launcher.clone();
        let retaining = control.clone();
        tokio::task::spawn_blocking(move || {
            super::launcher::retain(&data, &source, &launcher, &retaining)
        })
        .await
        .context("Launcher source retention stopped")??;
    }
    control.check()?;
    tracker
        .register_mele_package(
            &entry,
            &Record {
                writable_cache: false,
                version: if launcher.is_some() {
                    4
                } else if package.binary_approval.is_some() {
                    2
                } else {
                    1
                },
                target,
                package,
                launcher: launcher.map(|(entry, _)| entry),
            },
            &files,
            previous.is_some(),
        )
        .await?;
    Ok(AddResult {
        additional_mods: Vec::new(),
        mod_entry: entry,
        files_cached: files.len(),
        plugins_found: Vec::new(),
        warnings: Vec::new(),
    })
}

fn tracked_files(
    mod_id: &str,
    stored: &recipe::sources::StoredPackage,
    selected: &BTreeSet<String>,
) -> Result<Vec<ModFile>> {
    let (root, plan) = stored.selected_plan(selected)?;
    let mut files = BTreeMap::new();
    for file in plan.files {
        let original = display_path(&file.destination);
        files.insert(
            original.to_lowercase(),
            ModFile {
                mod_id: mod_id.to_string(),
                game_rel_lowercase: original.to_lowercase(),
                game_rel_original: original,
                cache_path: root.join(file.source).to_string_lossy().into_owned(),
            },
        );
    }
    for merge in plan.m3m {
        let cache_path = root
            .join(merge.source.relative)
            .to_string_lossy()
            .into_owned();
        for target in merge
            .files
            .into_iter()
            .flat_map(|file| file.target_candidates)
        {
            let original = format!("CookedPCConsole/{target}");
            files.insert(
                original.to_lowercase(),
                ModFile {
                    mod_id: mod_id.to_string(),
                    game_rel_lowercase: original.to_lowercase(),
                    game_rel_original: original,
                    cache_path: cache_path.clone(),
                },
            );
        }
    }
    Ok(files.into_values().collect())
}

fn recipe(
    target: Target,
    packages: Vec<Package>,
    launcher: Vec<super::launcher::Entry>,
    language: String,
    runtimes: bool,
) -> Recipe {
    let mut recipe = Recipe {
        version: if !launcher.is_empty() {
            4
        } else if packages
            .iter()
            .any(|package| package.binary_approval.is_some())
        {
            3
        } else {
            1
        },
        target,
        backend_version: 1,
        helper_version: Some(super::helper::protocol::VERSION.into()),
        language,
        packages,
        launcher,
        components: Vec::new(),
    };
    if runtimes {
        recipe.enable_runtime_components();
    }
    recipe
}

pub(crate) async fn remove(tracker: &Tracker, game: &Game, ids: &[String]) -> Result<()> {
    target(game)?;
    let control = Control::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let _lease = Lease::acquire(&control).await?;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    tracker.remove_mele_packages(&game.id, ids).await
}

pub(crate) async fn desired(
    tracker: &Tracker,
    game: &Game,
    language: String,
    purge: bool,
) -> Result<Recipe> {
    let target = target(game)?;
    let mut packages = Vec::new();
    let mut launcher = Vec::new();
    if !purge {
        for entry in tracker.list_mods(&game.id).await? {
            let record = tracker.mele_package(&entry.id).await?.with_context(|| {
                format!(
                    "Reinstall '{}' from its archive to create a MELE package recipe",
                    entry.name
                )
            })?;
            ensure!(
                record.target == target,
                "A MELE library entry targets another game"
            );
            let mut package = record.package;
            package.enabled = entry.enabled;
            if let Some(component) = record.launcher {
                launcher.push(component);
            }
            packages.push(package);
        }
    }
    let runtimes = packages.iter().any(|package| package.enabled);
    let recipe = recipe(target, packages, launcher, language, runtimes);
    recipe.validate()?;
    Ok(recipe)
}

#[cfg(test)]
mod tests;

fn display_path(path: &str) -> String {
    path.strip_prefix("~game~/")
        .map_or_else(|| path.into(), |path| format!("../{path}"))
}
