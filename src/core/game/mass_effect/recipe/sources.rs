use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use uuid::Uuid;

use crate::utils::paths;

use super::super::baseline::directory;
use super::super::journal::{Control, Identity, files};
use super::super::package::PackagePlan;
use super::{Abandoned, Package, Progress, Target, digest};

#[derive(Clone)]
pub(crate) struct StoredPackage {
    pub(super) root: PathBuf,
    pub(super) plan: PackagePlan,
    archive_sha256: Option<String>,
    original_plan: Option<PackagePlan>,
}

impl StoredPackage {
    pub(super) fn resolve(
        &mut self,
        selected: &BTreeSet<String>,
        context: &super::super::alternates::Context<'_>,
    ) -> Result<()> {
        if !self.plan.manifest.alternates.is_empty() {
            self.original_plan = Some(self.plan.clone());
            self.plan.resolve_context(&self.root, selected, context)?;
        }
        Ok(())
    }
    pub(crate) fn selection(&self) -> Package {
        Package {
            binary_approval: None,
            binary_files: Vec::new(),
            id: Uuid::new_v4().to_string(),
            source_sha256: self.plan.source_sha256.clone(),
            archive_sha256: self.archive_sha256.clone(),
            manifest_version: self.plan.manifest.format.clone(),
            mod_version: self.plan.manifest.version.clone(),
            enabled: true,
            options: Default::default(),
        }
    }

    pub(in crate::core::game::mass_effect) fn selected_plan(
        &self,
        selected: &BTreeSet<String>,
    ) -> Result<(PathBuf, PackagePlan)> {
        let mut plan = self.plan.clone();
        plan.resolve_selected(&self.root, selected)?;
        Ok((self.root.clone(), plan))
    }
}

pub(crate) async fn retain(
    source: PathBuf,
    plan: PackagePlan,
    archive_sha256: Option<String>,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<StoredPackage> {
    retain_in(
        source,
        plan,
        archive_sha256,
        paths::deployd_data_dir()?,
        cancelled,
        progress,
    )
    .await
}

pub(in crate::core::game::mass_effect) async fn retain_in(
    source: PathBuf,
    plan: PackagePlan,
    archive_sha256: Option<String>,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<StoredPackage> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(abandoned.clone());
    let control = Control::new(cancelled, abandoned);
    tokio::spawn(async move {
        let mutation = super::super::mutation_lock();
        let _mutation = tokio::select! {
            biased;
            _ = control.stopped() => anyhow::bail!("MELE source retention cancelled"),
            lease = mutation.lock_owned() => lease,
        };
        let _location = crate::core::location_recovery::activity_lock()
            .try_read_owned()
            .context(
                "Folder access is being changed; retry MELE source retention when it finishes",
            )?;
        tokio::task::spawn_blocking(move || {
            save(source, plan, archive_sha256, data, &control, &progress)
        })
        .await
        .context("MELE source retention file worker failed")?
    })
    .await
    .context("MELE source retention worker failed")?
}

fn root(data: &Path, sha256: &str) -> Result<PathBuf> {
    digest(sha256)?;
    Ok(data.join("mele-sources").join(sha256))
}

fn save(
    source: PathBuf,
    plan: PackagePlan,
    archive_sha256: Option<String>,
    data: PathBuf,
    control: &Control,
    progress: &Progress,
) -> Result<StoredPackage> {
    control.check()?;
    if let Some(hash) = &archive_sha256 {
        digest(hash)?;
    }
    directory(&source)?;
    ensure!(
        !plan.sources.is_empty() && plan.sources.len() <= 100_000,
        "Invalid MELE source inventory size"
    );
    ensure!(
        PackagePlan::inspect(&source, Some(plan.manifest.target))? == plan,
        "MELE package plan changed after preview; inspect the package again"
    );
    control.check()?;
    let destination = root(&data, &plan.source_sha256)?;
    let stored = StoredPackage {
        original_plan: None,
        root: destination.clone(),
        plan,
        archive_sha256,
    };
    match fs::symlink_metadata(&destination) {
        Ok(_) => {
            verify(&stored, control)?;
            return Ok(stored);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Cannot inspect retained MELE package"),
    }
    ensure!(
        !destination.starts_with(&source) && !source.starts_with(&destination),
        "MELE source storage must be separate from the imported package"
    );
    let parent = destination
        .parent()
        .context("Missing MELE source storage parent")?;
    files::create_directory(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".source-")
        .tempdir_in(parent)?;
    for (index, file) in stored.plan.sources.iter().enumerate() {
        control.check()?;
        files::copy(
            &source,
            &file.relative,
            stage.path(),
            &file.relative,
            &Identity {
                size: file.size,
                sha256: file.sha256.clone(),
            },
            control,
        )?;
        let path = stage.path().join(&file.relative);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
        fs::File::open(path)?.sync_all()?;
        progress(index + 1, stored.plan.sources.len());
    }
    let staged = StoredPackage {
        original_plan: None,
        root: stage.path().to_path_buf(),
        plan: stored.plan.clone(),
        archive_sha256: stored.archive_sha256.clone(),
    };
    verify(&staged, control)?;
    ensure!(
        PackagePlan::inspect(&source, Some(stored.plan.manifest.target))? == stored.plan,
        "MELE package changed while retaining sources"
    );
    control.check()?;
    fs::rename(stage.path(), &destination)
        .context("Cannot publish retained MELE sources; preserve existing storage and retry")?;
    files::sync(parent)?;
    verify(&stored, control)?;
    Ok(stored)
}

pub(in crate::core::game::mass_effect) fn load(
    data: &Path,
    package: &Package,
    target: Target,
    control: &Control,
) -> Result<StoredPackage> {
    control.check()?;
    let root = root(data, &package.source_sha256)?;
    directory(&root)
        .context("A MELE source package is unavailable; import the matching archive again")?;
    let plan = PackagePlan::inspect(&root, Some(target))?;
    ensure!(
        plan.source_sha256 == package.source_sha256
            && plan.manifest.format == package.manifest_version
            && plan.manifest.version == package.mod_version,
        "Retained MELE source identity or version differs from its recipe"
    );
    ensure!(
        !plan.needs_binary_approval()
            || package
                .binary_approval
                .as_ref()
                .is_some_and(|approval| approval.matches(&plan)),
        "This binary mod needs approval for its current source and game; reinstall it from its archive"
    );
    let stored = StoredPackage {
        original_plan: None,
        root,
        plan,
        archive_sha256: package.archive_sha256.clone(),
    };
    verify(&stored, control)?;
    Ok(stored)
}

pub(super) fn verify(stored: &StoredPackage, control: &Control) -> Result<()> {
    control.check()?;
    directory(&stored.root)?;
    ensure!(
        &PackagePlan::inspect(&stored.root, Some(stored.plan.manifest.target))?
            == stored.original_plan.as_ref().unwrap_or(&stored.plan),
        "Retained MELE sources changed; preserve them and import an intact source package"
    );
    for file in &stored.plan.sources {
        control.check()?;
        files::verify(
            &stored.root,
            &file.relative,
            Some(&Identity {
                size: file.size,
                sha256: file.sha256.clone(),
            }),
            control,
        )?;
        ensure!(
            fs::metadata(stored.root.join(&file.relative))?
                .permissions()
                .mode()
                & 0o222
                == 0,
            "Retained MELE sources must remain read-only"
        );
    }
    control.check()
}
