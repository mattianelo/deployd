use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use uuid::Uuid;

use crate::core::tracker::eclipse_packages::{EclipseComponent, EclipseInstall};
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::utils::paths;

use super::{AddModRequest, AddResult, DazipSource, cache, deployment};

fn component_for(
    source: &std::path::Path,
    destination: &std::path::Path,
    roots: &[DazipSource],
) -> EclipseComponent {
    if let Some(root) = roots.iter().find(|root| source.starts_with(&root.root)) {
        return EclipseComponent {
            kind: "dazip".into(),
            source_key: root.key.clone(),
        };
    }
    let lower = destination
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if lower.starts_with("addins/") {
        EclipseComponent {
            kind: "dazip".into(),
            source_key: "dazip:expanded".into(),
        }
    } else {
        EclipseComponent {
            kind: "override".into(),
            source_key: "override".into(),
        }
    }
}

pub(super) async fn add_components(request: AddModRequest<'_>) -> Result<AddResult> {
    let previous = if let Some(id) = request.replacing {
        Some(
            request
                .tracker
                .list_mods(&request.game.id)
                .await?
                .into_iter()
                .find(|entry| entry.id == id)
                .context("DAO component to replace no longer exists")?,
        )
    } else {
        None
    };
    let target = if let Some(previous) = &previous {
        request.tracker.eclipse_component(&previous.id).await?
    } else {
        None
    };
    let retained_files = if request.merging {
        request
            .tracker
            .get_mod_files(
                request
                    .replacing
                    .context("DAO merge needs an existing entry")?,
            )
            .await?
    } else {
        Vec::new()
    };
    let retained_component = target.clone().unwrap_or_else(|| EclipseComponent {
        kind: if retained_files
            .iter()
            .any(|file| file.game_rel_lowercase.starts_with("addins/"))
        {
            "dazip".into()
        } else {
            "override".into()
        },
        source_key: if retained_files
            .iter()
            .any(|file| file.game_rel_lowercase.starts_with("addins/"))
        {
            "dazip:expanded".into()
        } else {
            "override".into()
        },
    });
    let next_priority = request.tracker.next_priority(&request.game.id).await?;
    let game = request.game.clone();
    let name = request.mod_name.to_string();
    let roots = request.dazip_sources.to_vec();
    let cache_root = request.cache_root.to_path_buf();
    let excluded = request.excluded_files.clone();
    let old_entry = previous.clone();
    let prepared = tokio::task::spawn_blocking(move || -> Result<Vec<EclipseInstall>> {
        let rules = crate::core::rules::rules_for_game(&game.id);
        let files = super::filter_excluded_files(request.file_list, &rules, &game.engine, &game.data_subdir, &excluded);
        let mut groups: BTreeMap<String, (EclipseComponent, Vec<(PathBuf, PathBuf)>)> = BTreeMap::new();
        let mut selected_roots = BTreeMap::new();
        if request.merging {
            let retained = retained_files.iter().map(|file| (PathBuf::from(&file.cache_path), PathBuf::from(&file.game_rel_original))).collect();
            groups.insert(retained_component.source_key.clone(), (retained_component.clone(), retained));
        }
        for (source, destination) in files {
            let component = component_for(&source, &destination, &roots);
            if !request.merging
                && target.as_ref().is_some_and(|target| target.source_key != component.source_key)
            {
                continue;
            }
            if let Some(root) = roots.iter().find(|root| source.starts_with(&root.root)) {
                let selected = selected_roots.entry(root.key.clone()).or_insert_with(|| root.root.clone());
                ensure!(*selected == root.root, "More than one selected DAZIP uses add-in identity '{}'. Select only one version of that add-in.", root.key);
            }
            groups.entry(component.source_key.clone()).or_insert_with(|| (component, Vec::new())).1.push((source, destination));
        }
        ensure!(!groups.is_empty(), "The selected DAO component is absent from this archive or all its files were deselected");
        let multiple = groups.len() > 1;
        let mut installs = Vec::new();
        let mut created = Vec::new();
        let result = (|| -> Result<()> {
            for (index, (_, (mut component, files))) in groups.into_iter().enumerate() {
                let is_retained = request.merging && component.source_key == retained_component.source_key;
                let label = if !multiple || (!request.merging && target.is_some()) || is_retained {
                    name.clone()
                } else if component.kind == "override" {
                    format!("{name} (Overrides)")
                } else {
                    let source_name = component.source_key.strip_prefix("dazip:").unwrap_or(&component.source_key);
                    format!("{name} ({source_name})")
                };
                let id = Uuid::new_v4().to_string();
                let plan = deployment::route_and_plan(files, &roots, &game, &label, request.stripped_wrapper.as_deref(), &request.file_targets, &HashSet::new())?;
                let directory = paths::mod_cache_dir_in(&cache_root, &id);
                created.push(directory.clone());
                let cached = cache::write_files(&id, &directory, plan, request.on_progress.as_deref())?;
                if cached.mod_files.is_empty() {
                    std::fs::remove_dir_all(&directory)?;
                    continue;
                }
                if component.kind == "override" && !cached.mod_files.iter().any(|file| file.game_rel_lowercase.starts_with("packages/core/override/")) {
                    component.kind = "other".into();
                }
                let entry = ModEntry {
                    id,
                    game_id: game.id.clone(),
                    name: label,
                    archive_hash: if is_retained { old_entry.as_ref().and_then(|entry| entry.archive_hash.clone()) } else { request.archive_hash.clone() },
                    archive_path: if is_retained { old_entry.as_ref().and_then(|entry| entry.archive_path.clone()) } else { request.archive_path.clone() },
                    installed_at: Some(Utc::now().to_rfc3339()),
                    enabled: old_entry.as_ref().is_none_or(|entry| entry.enabled),
                    priority: old_entry.as_ref().map_or(next_priority, |entry| entry.priority) + index as i32,
                    nexus_mod_id: request.nexus_ids.as_ref().map(|ids| ids.mod_id).or_else(|| old_entry.as_ref().and_then(|entry| entry.nexus_mod_id)),
                    nexus_file_id: request.nexus_ids.as_ref().map(|ids| ids.file_id).or_else(|| old_entry.as_ref().and_then(|entry| entry.nexus_file_id)),
                    nexus_domain: request.nexus_ids.as_ref().map(|ids| ids.domain.clone()).or_else(|| old_entry.as_ref().and_then(|entry| entry.nexus_domain.clone())),
                    version: old_entry.as_ref().and_then(|entry| entry.version.clone()),
                    author: old_entry.as_ref().and_then(|entry| entry.author.clone()),
                    nexus_description: None,
                    latest_version: None,
                    nexus_file_name: None,
                    nexus_is_primary: false,
                    archive_md5: None,
                    install_target: InstallTarget::Data,
                    notes: old_entry.as_ref().and_then(|entry| entry.notes.clone()),
                };
                installs.push(EclipseInstall { entry, component, files: cached.mod_files });
            }
            ensure!(!installs.is_empty(), "No DAO resources selected for installation");
            Ok(())
        })();
        if let Err(error) = result {
            for path in created {
                if let Err(cleanup) = std::fs::remove_dir_all(&path) {
                    eprintln!("DAO staging cleanup failed: {cleanup}");
                }
            }
            return Err(error);
        }
        Ok(installs)
    }).await.context("DAO component staging failed")??;
    if let Err(error) = request
        .tracker
        .save_eclipse_install(&prepared, request.replacing)
        .await
    {
        let directories: Vec<_> = prepared
            .iter()
            .map(|install| paths::mod_cache_dir_in(request.cache_root, &install.entry.id))
            .collect();
        tokio::task::spawn_blocking(move || {
            for directory in directories {
                if let Err(error) = std::fs::remove_dir_all(directory) {
                    eprintln!("DAO staging cleanup failed: {error}");
                }
            }
        })
        .await
        .context("DAO cleanup task failed")?;
        return Err(error);
    }
    let count = prepared.iter().map(|install| install.files.len()).sum();
    let mut entries = prepared.into_iter().map(|install| install.entry);
    let first = entries
        .next()
        .context("DAO installation has no components")?;
    let mut warnings = Vec::new();
    if let Some(previous) = previous {
        let old_cache = paths::mod_cache_dir_in(request.cache_root, &previous.id);
        if let Some(warning) = tokio::task::spawn_blocking(move || {
            std::fs::remove_dir_all(&old_cache)
                .err()
                .filter(|error| error.kind() != std::io::ErrorKind::NotFound)
                .map(|error| format!("Could not remove replaced DAO cache: {error}"))
        })
        .await
        .context("DAO cache cleanup failed")?
        {
            warnings.push(warning);
        }
    }
    Ok(AddResult {
        mod_entry: first,
        additional_mods: entries.collect(),
        files_cached: count,
        plugins_found: Vec::new(),
        warnings,
    })
}

#[cfg(test)]
mod tests;
