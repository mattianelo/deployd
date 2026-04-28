use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;

use crate::core::tracker::Tracker;
use crate::models::backup::{BackupGameEntry, BackupManifest};
use crate::utils::paths;

const MANIFEST_VERSION: u32 = 1;
const MANIFEST_ENTRY: &str = "manifest.json";
const DB_ENTRY: &str = "deployd.db";

/// Build a `.deployd-backup` zip archive at `dest`.
///
/// The archive contains `manifest.json`, `deployd.db`, and one
/// `profiles/{game_id}/{profile_id}_{safe_name}.json` per profile.
pub async fn create_full_backup(dest: &Path, tracker: &Tracker) -> Result<BackupManifest> {
    // Flush WAL so the on-disk file is consistent before we copy it.
    sqlx::query("PRAGMA wal_checkpoint(FULL)")
        .execute(&tracker.pool)
        .await
        .context("WAL checkpoint failed")?;

    let db_bytes = tokio::fs::read(paths::db_path()?).await.context("Failed to read database")?;

    // Collect game-level metadata and export all profiles.
    let games_raw = tracker.load_persisted_games().await?;
    let mut game_entries: Vec<BackupGameEntry> = Vec::with_capacity(games_raw.len());
    // (game_id, profile_id, safe_name, json)
    let mut profile_files: Vec<(String, String, String, String)> = Vec::new();

    for game in &games_raw {
        let profiles = tracker.list_profiles(&game.id).await?;
        let mod_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE game_id = ?")
            .bind(&game.id)
            .fetch_one(&tracker.pool)
            .await
            .unwrap_or(0);

        game_entries.push(BackupGameEntry {
            id: game.id.clone(),
            title: game.title.clone(),
            profile_count: profiles.len(),
            mod_count: mod_count as usize,
        });

        for profile in &profiles {
            let export = tracker
                .export_profile(&profile.id)
                .await
                .with_context(|| format!("Failed to export profile {}", profile.id))?;
            let json = serde_json::to_string_pretty(&export)
                .context("Failed to serialize profile export")?;
            let safe_name = profile.name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            profile_files.push((game.id.clone(), profile.id.clone(), safe_name, json));
        }
    }

    let manifest = BackupManifest {
        version: MANIFEST_VERSION,
        created_at: Utc::now().to_rfc3339(),
        deployd_version: env!("CARGO_PKG_VERSION").to_string(),
        games: game_entries,
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).context("Failed to serialize manifest")?;

    // Write the zip on a blocking thread so we don't stall the GTK event loop.
    let dest = dest.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::create(&dest)
            .with_context(|| format!("Cannot create backup file at {}", dest.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file(MANIFEST_ENTRY, options)?;
        zip.write_all(manifest_json.as_bytes())?;

        zip.start_file(DB_ENTRY, options)?;
        zip.write_all(&db_bytes)?;

        for (game_id, profile_id, safe_name, json) in &profile_files {
            let entry_path = format!("profiles/{game_id}/{profile_id}_{safe_name}.json");
            zip.start_file(&entry_path, options)?;
            zip.write_all(json.as_bytes())?;
        }

        zip.finish()?;
        Ok(())
    })
    .await
    .context("Backup write task panicked")??;

    Ok(manifest)
}

/// Read only the manifest from a `.deployd-backup` archive (for preview).
pub fn read_backup_manifest(src: &Path) -> Result<BackupManifest> {
    let file = std::fs::File::open(src)
        .with_context(|| format!("Cannot open backup file at {}", src.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("Not a valid backup archive")?;
    let mut entry = archive
        .by_name(MANIFEST_ENTRY)
        .context("Backup archive is missing manifest.json")?;
    let mut json = String::new();
    entry.read_to_string(&mut json)?;
    let manifest: BackupManifest =
        serde_json::from_str(&json).context("manifest.json is malformed")?;
    if manifest.version != MANIFEST_VERSION {
        return Err(anyhow!(
            "Unsupported backup format version {}",
            manifest.version
        ));
    }
    Ok(manifest)
}

/// Stage a full DB restore from a `.deployd-backup` archive.
///
/// Writes the embedded `deployd.db` to the pending restore path. The current
/// database is backed up to `deployd.db.pre-restore` for one level of undo.
/// The pending file is consumed on the next app launch by `init.rs`.
pub fn stage_full_restore(src: &Path) -> Result<BackupManifest> {
    let manifest = read_backup_manifest(src)?;

    let db_path = paths::db_path()?;
    let pre_restore_path = db_path.with_extension("db.pre-restore");
    let pending_path = paths::pending_restore_path()?;

    // Back up the current DB (best-effort; don't abort if it doesn't exist yet).
    if db_path.exists() {
        std::fs::copy(&db_path, &pre_restore_path)
            .context("Failed to back up current database before restore")?;
    }

    // Extract deployd.db from the archive to the pending path.
    let file = std::fs::File::open(src)
        .with_context(|| format!("Cannot open backup file at {}", src.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("Not a valid backup archive")?;
    let mut db_entry = archive
        .by_name(DB_ENTRY)
        .context("Backup archive is missing deployd.db")?;

    let mut pending_file = std::fs::File::create(&pending_path)
        .with_context(|| format!("Cannot create pending restore file at {}", pending_path.display()))?;
    std::io::copy(&mut db_entry, &mut pending_file)
        .context("Failed to write pending restore database")?;

    Ok(manifest)
}

/// Import all profiles for `game_id` from a `.deployd-backup` archive into the
/// current install. Uses the existing `import_profile()` logic (name-matching).
/// Returns the names of successfully imported profiles.
pub async fn import_profiles_from_backup(
    src: &Path,
    game_id: &str,
    tracker: &Tracker,
) -> Result<Vec<String>> {
    let src = src.to_path_buf();
    let game_id = game_id.to_string();
    let game_id_for_closure = game_id.clone();

    // Extract matching profile JSON bytes on a blocking thread.
    let profile_jsons: Vec<String> = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
        let file = std::fs::File::open(&src)
            .with_context(|| format!("Cannot open backup file at {}", src.display()))?;
        let mut archive = zip::ZipArchive::new(file).context("Not a valid backup archive")?;
        let prefix = format!("profiles/{game_id_for_closure}/");
        let names: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                let entry = archive.by_index(i).ok()?;
                if entry.name().starts_with(&prefix) && entry.name().ends_with(".json") {
                    Some(entry.name().to_string())
                } else {
                    None
                }
            })
            .collect();

        let mut jsons = Vec::with_capacity(names.len());
        for name in names {
            let mut entry = archive.by_name(&name)?;
            let mut json = String::new();
            entry.read_to_string(&mut json)?;
            jsons.push(json);
        }
        Ok(jsons)
    })
    .await
    .context("Profile extraction task panicked")??;

    let mut imported = Vec::with_capacity(profile_jsons.len());
    for json in &profile_jsons {
        let export: crate::models::profile_export::ProfileExport =
            serde_json::from_str(json).context("Malformed profile JSON in backup")?;
        let name = export.profile_name.clone();
        tracker
            .import_profile(&game_id, &export)
            .await
            .with_context(|| format!("Failed to import profile \"{name}\""))?;
        imported.push(name);
    }

    Ok(imported)
}
