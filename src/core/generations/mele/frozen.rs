use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::*;
use crate::core::game::mass_effect::{
    library::Record,
    package::{PackagePlan, SourceFile, tree_digest},
    recipe::Recipe,
};
use crate::core::generations::manifest::Source;
use crate::core::generations::records::{Table, text};
use crate::core::generations::store::Store;

pub(super) async fn sources(
    history: &History,
    manifest: &Manifest,
    mut recipe: Recipe,
    control: Control,
) -> Result<(tempfile::TempDir, Manifest, Recipe)> {
    let store = history.store.clone();
    let cache = history.cache.clone();
    let mut manifest = manifest.clone();
    history
        .lease
        .blocking(move || -> Result<_> {
            manifest.validate()?;
            let root = tempfile::Builder::new()
                .prefix(".mele-sources-")
                .tempdir_in(cache)?;
            let ordering = manifest
                .records
                .iter()
                .find(|rows| rows.table == Table::ProfileMods)
                .context("Missing frozen MELE order")?
                .rows
                .iter()
                .map(|row| {
                    Ok((
                        text(row, "mod_id")?.to_owned(),
                        (
                            row.get("priority")
                                .and_then(Value::as_i64)
                                .context("Missing frozen mod priority")?,
                            row.get("enabled")
                                .and_then(Value::as_i64)
                                .context("Missing frozen enabled state")?
                                != 0,
                        ),
                    ))
                })
                .collect::<Result<BTreeMap<_, _>>>()?;
            let mut packages = Vec::new();
            for row in &mut manifest
                .records
                .iter_mut()
                .find(|rows| rows.table == Table::MelePackages)
                .context("Missing frozen MELE packages")?
                .rows
            {
                let mut record: Record = serde_json::from_str(text(row, "document")?)?;
                let (priority, enabled) = ordering
                    .get(&record.package.id)
                    .context("Frozen MELE package has no profile order")?;
                record.package.enabled = *enabled;
                prepare_package(
                    &store,
                    root.path(),
                    &manifest.sources,
                    &mut record,
                    &control,
                )?;
                row.insert(
                    "document".into(),
                    Value::String(serde_json::to_string(&record)?),
                );
                packages.push((*priority, record.package));
            }
            packages.sort_by(|(left, a), (right, b)| left.cmp(right).then_with(|| a.id.cmp(&b.id)));
            recipe.packages = packages.into_iter().map(|(_, package)| package).collect();
            ensure!(
                recipe.target.game_id() == manifest.game_id,
                "Frozen MELE recipe targets another game"
            );
            Ok((root, manifest, recipe))
        })
        .await
        .context("Frozen MELE source materialization worker stopped")?
}

fn prepare_package(
    store: &Store,
    root: &Path,
    sources: &[Source],
    record: &mut Record,
    control: &Control,
) -> Result<()> {
    record.validate()?;
    let anchor = format!("cache/{}", record.package.id);
    let prefix = format!("{anchor}/");
    let sources: Vec<_> = sources
        .iter()
        .filter(|source| source.path == anchor || source.path.starts_with(&prefix))
        .collect();
    let mut files: Vec<_> = sources
        .iter()
        .filter_map(|source| {
            source.content.as_ref().map(|identity| SourceFile {
                relative: source
                    .path
                    .strip_prefix(&prefix)
                    .unwrap_or_default()
                    .to_owned(),
                size: identity.size,
                sha256: identity.sha256.clone(),
            })
        })
        .collect();
    files.sort_by(|a, b| a.relative.cmp(&b.relative));
    let unchanged_disabled = !record.package.enabled
        && !files.is_empty()
        && tree_digest(&files) == record.package.source_sha256;
    for source in sources {
        control.check()?;
        if unchanged_disabled {
            if let Some(identity) = &source.content {
                store.verify(identity, control)?;
            }
        } else {
            let path = root.join(&source.path);
            if let Some(identity) = &source.content {
                fs::create_dir_all(path.parent().context("Frozen source has no parent")?)?;
                store.materialize(identity, &path, 0o400, control)?;
            } else {
                fs::create_dir_all(path)?;
            }
        }
    }
    if unchanged_disabled {
        return control.check();
    }
    let directory = root.join(anchor);
    let plan = PackagePlan::inspect(&directory, Some(record.target))?;
    ensure!(
        record.package.binary_approval.is_none()
            || record.package.source_sha256 == plan.source_sha256,
        "Edited binary-mod sources require renewed component consent before preparing deployment"
    );
    record.package.source_sha256 = plan.source_sha256;
    record.package.manifest_version = plan.manifest.format;
    record.package.mod_version = plan.manifest.version;
    record.validate()?;
    let destination = root
        .join("mele-sources")
        .join(&record.package.source_sha256);
    fs::create_dir_all(
        destination
            .parent()
            .context("Frozen package has no parent")?,
    )?;
    if !destination.try_exists()? {
        fs::rename(directory, destination)?;
    }
    control.check()
}

#[cfg(test)]
mod tests;
