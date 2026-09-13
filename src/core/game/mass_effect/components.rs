use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use tempfile::TempDir;

use super::Target;
use super::baseline::{Baseline, originals::Preserved};
use super::journal::{self, Identity, State};
use super::operation::{Control, Lease};
use super::package::SourceFile;

mod artifacts;
mod catalog;
pub(super) use catalog::{BINK, ORIGINAL, binary_runtime, texture_runtime};
pub(crate) use catalog::{Component, Selection, required};

pub(super) struct Plan {
    pub(super) selections: Vec<Selection>,
    pub(super) files: Vec<SourceFile>,
}

pub(super) fn validate(selections: &[Selection], target: Target) -> Result<()> {
    catalog::validate(selections, target)
}

pub(super) fn inventory(
    selections: &[Selection],
    baseline: &Baseline,
    target: Target,
) -> Result<Vec<SourceFile>> {
    validate(selections, target)?;
    let mut files = Vec::new();
    for selected in selections {
        files.extend(
            selected
                .component
                .payloads()
                .iter()
                .map(|payload| payload.file()),
        );
        if selected.component == Component::BinkProxy {
            let original = baseline.files.iter().find(|file| file.relative == BINK)
                .context("MELE runtime installation requires the original game Bink library in the restoration baseline")?;
            ensure!(
                original.size > 0 && original.sha256 != Component::BinkProxy.artifact().sha256,
                "The restoration baseline contains a Bink proxy; restore the game before configuring MELE runtimes"
            );
            files.push(SourceFile {
                relative: ORIGINAL.into(),
                size: original.size,
                sha256: original.sha256.clone(),
            });
        }
    }
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(files)
}

impl Plan {
    pub(super) fn inspect(
        selections: &[Selection],
        baseline: &Baseline,
        target: Target,
    ) -> Result<Self> {
        let files = inventory(selections, baseline, target)?;
        for file in &files {
            if file.relative != BINK
                && let Some(existing) = baseline
                    .files
                    .iter()
                    .find(|entry| entry.relative.eq_ignore_ascii_case(&file.relative))
            {
                ensure!(
                    existing.relative == file.relative
                        && existing.size == file.size
                        && existing.sha256 == file.sha256,
                    "An unmanaged runtime occupies '{}'; reconcile it before installing MELE components",
                    file.relative
                );
            }
        }
        Ok(Self {
            selections: selections.to_vec(),
            files,
        })
    }

    pub(super) fn needs_original(&self) -> bool {
        self.selections
            .iter()
            .any(|s| s.component == Component::BinkProxy)
    }
}

pub(super) fn owned(state: &State, baseline: &Baseline, target: Target) -> Result<Vec<SourceFile>> {
    let selections = state
        .recipe
        .as_ref()
        .map(|recipe| recipe.components.as_slice())
        .unwrap_or_default();
    let expected = inventory(selections, baseline, target)?;
    for file in &expected {
        ensure!(
            state.files.iter().any(|entry| entry == file),
            "MELE component ownership does not match its pinned file inventory"
        );
    }
    Ok(expected)
}

pub(super) fn preflight(
    root: &Path,
    plan: &Plan,
    previous: Option<&State>,
    baseline: &Baseline,
    target: Target,
    repair: bool,
    control: &Control,
) -> Result<()> {
    let previous_files = previous
        .map(|state| owned(state, baseline, target))
        .transpose()?
        .unwrap_or_default();
    if plan.files.is_empty() && previous_files.is_empty() {
        return Ok(());
    }
    let paths: BTreeSet<_> = plan
        .files
        .iter()
        .chain(&previous_files)
        .map(|file| &file.relative)
        .collect();
    for path in paths {
        control.check()?;
        let before = previous
            .and_then(|state| state.files.iter().find(|file| &file.relative == path))
            .map(|file| Identity {
                size: file.size,
                sha256: file.sha256.clone(),
            })
            .or_else(|| {
                baseline
                    .files
                    .iter()
                    .find(|file| &file.relative == path)
                    .map(|file| Identity {
                        size: file.size,
                        sha256: file.sha256.clone(),
                    })
            });
        if repair
            && previous_files.iter().any(|file| &file.relative == path)
            && std::fs::symlink_metadata(root.join(path))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            journal::files::verify(root, path, None, control)?;
        } else {
            journal::files::verify(root, path, before.as_ref(), control)
                .with_context(|| format!("MELE runtime '{path}' changed or is unmanaged; preserve it and reconcile before installation or removal"))?;
        }
    }
    let binary_files = previous
        .map(super::binary::owned)
        .transpose()?
        .unwrap_or_default();
    let asi = root.join("Binaries/Win64/ASI");
    let exists = match std::fs::symlink_metadata(&asi) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).context(
                "Cannot inspect the MELE ASI folder; restore folder access before retrying",
            );
        }
    };
    if exists {
        for entry in walkdir::WalkDir::new(&asi).follow_links(false) {
            control.check()?;
            let entry = entry?;
            ensure!(
                entry.file_type().is_file() || entry.file_type().is_dir(),
                "MELE ASI folders contain a link or special file; reconcile it before installing runtime components"
            );
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("asi") || ext.eq_ignore_ascii_case("dll")
                })
            {
                let path = entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("Invalid ASI path")?;
                ensure!(
                    previous_files
                        .iter()
                        .chain(&binary_files)
                        .any(|file| file.relative == path)
                        || baseline.files.iter().any(|file| file.relative == path
                            && plan
                                .files
                                .iter()
                                .any(|planned| planned.relative == path
                                    && planned.sha256 == file.sha256)),
                    "An unmanaged ASI is installed; reconcile existing plugins before installing MELE runtimes"
                );
            }
        }
    }
    Ok(())
}

pub(super) struct Prepared {
    directory: TempDir,
    pub(super) files: Vec<SourceFile>,
}

impl Prepared {
    pub(super) fn append(
        self,
        root: &Path,
        files: &mut Vec<SourceFile>,
        control: &Control,
    ) -> Result<()> {
        for file in self.files {
            journal::files::copy(
                self.directory.path(),
                &file.relative,
                root,
                &file.relative,
                &Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
                control,
            )?;
            files.push(file);
        }
        files.sort_by(|a, b| a.relative.cmp(&b.relative));
        Ok(())
    }
}

pub(super) async fn prepare(
    plan: Plan,
    originals: Option<Preserved>,
    data: PathBuf,
    control: Control,
    progress: artifacts::Progress,
    lease: Arc<Lease>,
) -> Result<Prepared> {
    let mut inputs = BTreeMap::new();
    let count = plan.selections.len();
    for (index, selected) in plan.selections.iter().enumerate() {
        let report = progress.clone();
        let bytes = artifacts::load(
            selected.component.artifact(),
            data.clone(),
            control.clone(),
            Arc::new(move |done, total| report((index * 1000 + done * 1000 / total) / count, 1000)),
        )
        .await?;
        inputs.insert(selected.component, bytes);
    }
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        control.check()?;
        let parent = data.join("mele-components").join("staging");
        journal::files::create_directory(&parent)?;
        let directory = tempfile::Builder::new()
            .prefix("runtime-")
            .tempdir_in(parent)?;
        for (component, bytes) in inputs {
            for (payload, bytes) in component
                .payloads()
                .iter()
                .zip(artifacts::payloads(component, &bytes, &control)?)
            {
                artifacts::verify(&bytes, payload.size, payload.sha256)?;
                artifacts::write(directory.path(), &payload.file().relative, &bytes, &control)?;
            }
        }
        if let Some(backing) = plan.files.iter().find(|file| file.relative == ORIGINAL) {
            journal::files::copy(
                &originals
                    .as_ref()
                    .context("Bink runtime requires preserved game-owned backing")?
                    .root,
                BINK,
                directory.path(),
                ORIGINAL,
                &Identity {
                    size: backing.size,
                    sha256: backing.sha256.clone(),
                },
                &control,
            )?;
        }
        control.check()?;
        Ok(Prepared {
            directory,
            files: plan.files,
        })
    })
    .await
    .context("MELE runtime preparation worker failed")?
}

#[cfg(test)]
pub(super) mod test_cache;

#[cfg(test)]
mod tests;

pub(super) fn proxy_identity() -> Identity {
    let artifact = Component::BinkProxy.artifact();
    Identity {
        size: artifact.size,
        sha256: artifact.sha256.into(),
    }
}

pub(super) async fn cache_proxy(data: PathBuf, control: Control) -> Result<()> {
    artifacts::load(
        Component::BinkProxy.artifact(),
        data,
        control,
        Arc::new(|_, _| {}),
    )
    .await?;
    Ok(())
}

pub(super) fn reserved(path: &str) -> bool {
    path.eq_ignore_ascii_case(ORIGINAL)
        || path.strip_prefix("Binaries/Win64/").is_some_and(|name| {
            name.to_ascii_lowercase().starts_with("oo2core_")
                && name.to_ascii_lowercase().ends_with(".dll")
        })
        || [
            Component::BinkProxy,
            Component::AutoToc,
            Component::Le1Autoload,
            Component::VisualCpp,
            Component::Le1TextureOverride,
            Component::Le2TextureOverride,
            Component::Le3TextureOverride,
        ]
        .iter()
        .flat_map(|component| component.payloads())
        .any(|payload| payload.file().relative.eq_ignore_ascii_case(path))
}
