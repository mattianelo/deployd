use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::models::manifest::ModFile;

use super::deployment::{PlannedAction, PlannedFile};

pub(super) struct CachedDeployment {
    pub mod_files: Vec<ModFile>,
    pub plugin_cache_files: Vec<(String, PathBuf)>,
}

pub(super) fn write_files(
    mod_id: &str,
    cache_dir: &Path,
    plan: Vec<PlannedFile>,
    on_progress: Option<&(dyn Fn(usize, usize) + Send)>,
) -> Result<CachedDeployment> {
    fs::create_dir_all(cache_dir)
        .with_context(|| format!("Cannot create cache dir: {}", cache_dir.display()))?;

    let total_files = plan.len();
    let mut mod_files = Vec::with_capacity(total_files);
    let mut plugin_cache_files = Vec::new();
    for (file_idx, planned) in plan.into_iter().enumerate() {
        match planned.action {
            PlannedAction::Skip => {}
            PlannedAction::Directory { explicit_root } => {
                let cache_sentinel = cache_dir.join(&planned.lowercase_rel);
                if let Err(error) = fs::create_dir_all(&cache_sentinel) {
                    eprintln!(
                        "[deployd] WARNING: cannot create cache sentinel '{}': {error}",
                        cache_sentinel.display()
                    );
                }
                let lowercase = planned.lowercase_rel.to_string_lossy();
                let (recorded_rel, original_recorded_rel) = if explicit_root {
                    (
                        format!("../{lowercase}/"),
                        format!("../{}/", planned.original_rel),
                    )
                } else {
                    (
                        format!("{lowercase}/"),
                        format!("{}/", planned.original_rel),
                    )
                };
                mod_files.push(ModFile {
                    mod_id: mod_id.to_string(),
                    game_rel_lowercase: recorded_rel,
                    game_rel_original: original_recorded_rel,
                    cache_path: cache_sentinel.to_string_lossy().to_string(),
                });
            }
            PlannedAction::File {
                deploy_to_root,
                plugin_name,
            } => {
                let cache_file = cache_dir.join(&planned.lowercase_rel);
                if let Some(parent) = cache_file.parent() {
                    fs::create_dir_all(parent)?;
                }
                match fs::copy(&planned.source, &cache_file) {
                    Ok(_) => {}
                    Err(error) if error.raw_os_error() == Some(21) => {
                        eprintln!(
                            "[deployd] WARNING: skipping '{}' — resolved as directory at copy time (EISDIR)",
                            planned.source.display()
                        );
                        if let Some(callback) = on_progress {
                            callback(file_idx + 1, total_files);
                        }
                        continue;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("Cache copy failed: {}", planned.source.display())
                        });
                    }
                }

                let lowercase = planned.lowercase_rel.to_string_lossy();
                let recorded_rel = if deploy_to_root {
                    format!("../{lowercase}")
                } else {
                    lowercase.to_string()
                };
                let original_recorded_rel = if deploy_to_root {
                    format!("../{}", planned.original_rel)
                } else {
                    planned.original_rel
                };
                if let Some(plugin_name) = plugin_name {
                    plugin_cache_files.push((plugin_name, cache_file.clone()));
                }
                mod_files.push(ModFile {
                    mod_id: mod_id.to_string(),
                    game_rel_lowercase: recorded_rel,
                    game_rel_original: original_recorded_rel,
                    cache_path: cache_file.to_string_lossy().to_string(),
                });
            }
        }
        if let Some(callback) = on_progress {
            callback(file_idx + 1, total_files);
        }
    }

    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(mod_files.len());
    for file in mod_files.into_iter().rev() {
        if seen.insert(file.game_rel_lowercase.clone()) {
            deduped.push(file);
        }
    }
    deduped.reverse();

    Ok(CachedDeployment {
        mod_files: deduped,
        plugin_cache_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_planned_root_plugin_and_records_cache_path() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("Plugin.esp");
        fs::write(&source, b"plugin")?;
        let cache_dir = temp.path().join("cache");
        let cached = write_files(
            "mod-id",
            &cache_dir,
            vec![PlannedFile {
                source,
                lowercase_rel: PathBuf::from("plugin.esp"),
                original_rel: "Plugin.esp".to_string(),
                action: PlannedAction::File {
                    deploy_to_root: true,
                    plugin_name: Some("Plugin.esp".to_string()),
                },
            }],
            None,
        )?;

        assert_eq!(fs::read(cache_dir.join("plugin.esp"))?, b"plugin");
        assert_eq!(cached.mod_files[0].game_rel_lowercase, "../plugin.esp");
        assert_eq!(cached.mod_files[0].game_rel_original, "../Plugin.esp");
        assert_eq!(cached.plugin_cache_files[0].0, "Plugin.esp");
        Ok(())
    }

    #[test]
    fn records_planned_directory_sentinel_with_trailing_separator() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("Domains");
        fs::create_dir(&source)?;
        let cache_dir = temp.path().join("cache");
        let cached = write_files(
            "mod-id",
            &cache_dir,
            vec![PlannedFile {
                source,
                lowercase_rel: PathBuf::from("domains"),
                original_rel: "Domains".to_string(),
                action: PlannedAction::Directory {
                    explicit_root: false,
                },
            }],
            None,
        )?;

        assert!(cache_dir.join("domains").is_dir());
        assert_eq!(cached.mod_files[0].game_rel_lowercase, "domains/");
        assert_eq!(cached.mod_files[0].game_rel_original, "Domains/");
        Ok(())
    }
}
