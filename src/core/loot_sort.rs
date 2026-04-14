use std::{collections::HashMap, path::PathBuf};

use anyhow::Context;

use crate::models::plugin::PluginDirtyInfo;

fn loot_game_type(game_id: &str) -> Option<libloot::GameType> {
    match game_id {
        "skyrimse" => Some(libloot::GameType::SkyrimSE),
        "fallout4" => Some(libloot::GameType::Fallout4),
        "falloutnv" => Some(libloot::GameType::FalloutNV),
        _ => None,
    }
}

/// Returns `true` if the given game ID is supported by the LOOT sort / dirty-check integration.
/// Used by `ToolExited` to decide whether to auto-trigger a re-sort.
pub fn game_has_loot_support(game_id: &str) -> bool {
    loot_game_type(game_id).is_some()
}

fn loot_game_folder(game_id: &str) -> Option<&'static str> {
    match game_id {
        "skyrimse" => Some("Skyrim Special Edition"),
        "fallout4" => Some("Fallout 4"),
        "falloutnv" => Some("Fallout New Vegas"),
        _ => None,
    }
}

fn loot_masterlist_repo(game_id: &str) -> Option<&'static str> {
    match game_id {
        "skyrimse" => Some("skyrimse"),
        "fallout4" => Some("fallout4"),
        "falloutnv" => Some("falloutnv"),
        _ => None,
    }
}

/// Search well-known LOOT config directories for an existing masterlist.
fn find_installed_masterlist(game_id: &str) -> Option<PathBuf> {
    let folder = loot_game_folder(game_id)?;

    // Flatpak LOOT (io.github.loot.loot)
    if let Some(home) = dirs::home_dir() {
        let p = home
            .join(".var/app/io.github.loot.loot/config/LOOT/games")
            .join(folder)
            .join("masterlist.yaml");
        if p.exists() {
            return Some(p);
        }
    }

    // Native LOOT
    if let Some(config) = dirs::config_dir() {
        let p = config
            .join("LOOT/games")
            .join(folder)
            .join("masterlist.yaml");
        if p.exists() {
            return Some(p);
        }
    }

    None
}

fn cached_masterlist_path(game_id: &str) -> anyhow::Result<PathBuf> {
    let config = dirs::config_dir().context("No config directory available")?;
    Ok(config
        .join("deployd/loot")
        .join(game_id)
        .join("masterlist.yaml"))
}

/// Return an existing masterlist path, downloading from GitHub if needed.
async fn ensure_masterlist(game_id: &str) -> anyhow::Result<PathBuf> {
    if let Some(p) = find_installed_masterlist(game_id) {
        return Ok(p);
    }

    let cached = cached_masterlist_path(game_id)?;
    if cached.exists() {
        return Ok(cached);
    }

    let repo = loot_masterlist_repo(game_id)
        .ok_or_else(|| anyhow::anyhow!("No masterlist repository known for game '{game_id}'"))?;
    let url = format!("https://raw.githubusercontent.com/loot/{repo}/v0.21/masterlist.yaml");

    let bytes = reqwest::get(&url)
        .await
        .context("Failed to download LOOT masterlist")?
        .error_for_status()
        .context("LOOT masterlist download returned an error status")?
        .bytes()
        .await
        .context("Failed to read LOOT masterlist response body")?;

    if let Some(parent) = cached.parent() {
        std::fs::create_dir_all(parent).context("Failed to create masterlist cache directory")?;
    }
    std::fs::write(&cached, bytes).context("Failed to write cached masterlist")?;

    Ok(cached)
}

/// Compute the CRC-32 checksum of a file by reading its full contents.
/// LOOT uses standard CRC-32 (Ethernet/ZIP polynomial) to fingerprint plugins;
/// this matches what libloot stores in the masterlist dirty-info entries.
fn compute_file_crc32(path: &std::path::Path) -> Option<u32> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize())
}

/// Sort `plugin_filenames` using libloot and the LOOT masterlist.
///
/// - `game_id`          — Deployd game identifier (e.g. `"skyrimse"`).
/// - `game_path`        — Game install directory (root, not Data/).
/// - `data_subdir`      — Subdirectory that holds plugin files (e.g. `"Data"`).
/// - `plugin_filenames` — Plugin filenames to sort (e.g. `["Skyrim.esm", "MyMod.esp"]`).
/// - `local_data_path`  — AppData local dir used by libloadorder; derived from
///   the Wine prefix for Heroic installs. Falls back to `game_path`.
///
/// Returns `(sorted_filenames, dirty_info)` where `dirty_info` maps lowercase filenames
/// to their dirty-edit summary (ITM, UDR, NAV counts + cleaning utility name).
/// Once the user cleans a plugin the CRC changes and it is automatically removed from
/// the dirty map on the next LOOT sort.
pub async fn sort_plugins(
    game_id: &str,
    game_path: PathBuf,
    data_subdir: String,
    plugin_filenames: Vec<String>,
    local_data_path: Option<PathBuf>,
) -> anyhow::Result<(Vec<String>, HashMap<String, PluginDirtyInfo>)> {
    let game_type = loot_game_type(game_id)
        .ok_or_else(|| anyhow::anyhow!("LOOT does not support game '{game_id}'"))?;

    let masterlist_path = ensure_masterlist(game_id).await?;

    let local_path = local_data_path.unwrap_or_else(|| game_path.clone());

    tokio::task::spawn_blocking(
        move || -> anyhow::Result<(Vec<String>, HashMap<String, PluginDirtyInfo>)> {
            let mut game_handle =
                libloot::Game::with_local_path(game_type, &game_path, &local_path)
                    .context("Failed to create libloot game handle")?;

            // Load the LOOT masterlist into the database object.
            {
                let db = game_handle.database();
                let mut db_w = db.write().expect("libloot database RwLock poisoned");
                db_w.load_masterlist(&masterlist_path)
                    .context("Failed to load LOOT masterlist")?;
            }

            // Load plugin headers for files that are currently deployed on disk.
            // Deployd always deploys with lowercase filenames; try the lowercase path first
            // so plugins like "Be Exceptional.esp" are found as "be exceptional.esp".
            let data_path = game_path.join(&data_subdir);
            let plugin_paths: Vec<PathBuf> = plugin_filenames
                .iter()
                .map(|f| {
                    let lower = data_path.join(f.to_lowercase());
                    if lower.exists() {
                        lower
                    } else {
                        data_path.join(f)
                    }
                })
                .filter(|p| p.exists())
                .collect();
            let refs: Vec<&std::path::Path> = plugin_paths.iter().map(PathBuf::as_path).collect();
            if !refs.is_empty() {
                game_handle
                    .load_plugin_headers(&refs)
                    .context("Failed to load plugin headers")?;
            }

            // Derive sort names from the paths that were actually loaded so that the
            // names passed to sort_plugins() exactly match what libloot has in its cache.
            let loaded_names: Vec<String> = plugin_paths
                .iter()
                .filter_map(|p| p.file_name()?.to_str())
                .map(str::to_owned)
                .collect();

            // Detect dirty plugins: match each plugin's on-disk CRC against dirty entries
            // from the masterlist. CRC-based matching means a cleaned plugin automatically
            // loses the dirty flag on the next sort (its CRC changes after cleaning).
            //
            // NOTE: load_plugin_headers() does NOT compute file CRCs — Plugin::crc() returns
            // None after a headers-only load. We compute CRC-32 directly from the file
            // contents so we don't need to call the slower load_plugins().
            let plugin_crcs: Vec<(String, u32)> = plugin_paths
                .iter()
                .filter_map(|path| {
                    let name = path.file_name()?.to_str()?.to_owned();
                    let crc = compute_file_crc32(path)?;
                    Some((name, crc))
                })
                .collect();

            // Build dirty-info map: lowercase filename → ITM/UDR/NAV counts + utility name.
            // Uses the matching dirty entry whose CRC equals the plugin's on-disk CRC.
            let dirty_info: HashMap<String, PluginDirtyInfo> = {
                let db = game_handle.database();
                let db_r = db.read().expect("libloot database RwLock poisoned");
                plugin_crcs
                    .iter()
                    .filter_map(|(name, file_crc)| {
                        let Ok(Some(metadata)) = db_r.plugin_metadata(
                            name,
                            libloot::MergeMode::WithoutUserMetadata,
                            libloot::EvalMode::DoNotEvaluate,
                        ) else {
                            return None;
                        };
                        let entry = metadata
                            .dirty_info()
                            .iter()
                            .find(|d| d.crc() == *file_crc)?;
                        Some((
                            name.to_lowercase(),
                            PluginDirtyInfo {
                                itm: entry.itm_count(),
                                udr: entry.deleted_reference_count(),
                                nav: entry.deleted_navmesh_count(),
                                cleaning_utility: entry.cleaning_utility().to_string(),
                            },
                        ))
                    })
                    .collect()
            };

            let name_refs: Vec<&str> = loaded_names.iter().map(String::as_str).collect();
            let sorted = game_handle
                .sort_plugins(&name_refs)
                .map_err(|e| anyhow::anyhow!("LOOT sort failed: {e}"))?;

            Ok((sorted, dirty_info))
        },
    )
    .await
    .context("LOOT sort task panicked")
    .and_then(|inner| inner)
}
