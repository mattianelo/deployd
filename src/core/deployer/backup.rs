use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::core::game::eclipse::DOCS_PREFIX;
use crate::core::tracker::Tracker;
use crate::core::tracker::vanilla_backups::VanillaBackupRecord;
use crate::models::game::Game;
use crate::utils::paths;

use super::filesystem::{find_existing_deploy_path_case_insensitive, resolve_deploy_path};

#[derive(Debug, Default)]
pub(super) struct RestoreSummary {
    pub(super) restored: usize,
    pub(super) warnings: Vec<String>,
}

pub(super) async fn bake_modified_plugins(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let plugin_files = tracker
        .get_deployed_plugin_files(&game.id)
        .await
        .context("Failed to load deployed plugins before synchronization")?;
    for (_, ref game_rel_orig, ref cache_path) in plugin_files {
        let disk_path = resolve_deploy_path(game_rel_orig, &game.path, game_data)
            .with_context(|| format!("Invalid deployed plugin path '{game_rel_orig}'"))?;
        if !disk_path
            .try_exists()
            .with_context(|| format!("Failed to inspect deployed plugin '{game_rel_orig}'"))?
            || !cache_path
                .try_exists()
                .with_context(|| format!("Failed to inspect cached plugin '{game_rel_orig}'"))?
        {
            continue;
        }
        let disk_ino = fs::metadata(&disk_path)
            .with_context(|| format!("Failed to inspect deployed plugin '{game_rel_orig}'"))?
            .ino();
        let cache_ino = fs::metadata(cache_path)
            .with_context(|| format!("Failed to inspect cached plugin '{game_rel_orig}'"))?
            .ino();
        if disk_ino != cache_ino {
            fs::copy(&disk_path, cache_path).with_context(|| {
                format!("Could not preserve modified plugin '{game_rel_orig}' in the mod cache")
            })?;
        }
    }
    Ok(())
}

pub(super) async fn restore_all_vanilla(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
) -> Result<RestoreSummary> {
    let backups = tracker
        .get_all_vanilla_backups(&game.id)
        .await
        .context("Failed to load vanilla backup records")?;
    let mut summary = RestoreSummary::default();
    for (relative, backup_path) in backups {
        let record = VanillaBackupRecord {
            game_rel_path: relative.clone(),
            backup_path,
        };
        restore_vanilla_file(game, tracker, game_data, &relative, record, &mut summary).await;
    }
    Ok(summary)
}

pub(super) async fn restore_vanilla_for_paths(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
    canonical_paths: &[String],
) -> Result<RestoreSummary> {
    let mut summary = RestoreSummary::default();
    for canonical_path in canonical_paths {
        let record = tracker
            .get_vanilla_backup(&game.id, canonical_path)
            .await
            .with_context(|| {
                format!("Failed to load vanilla backup record for '{canonical_path}'")
            })?;
        if let Some(record) = record {
            restore_vanilla_file(
                game,
                tracker,
                game_data,
                canonical_path,
                record,
                &mut summary,
            )
            .await;
        }
    }
    Ok(summary)
}

async fn restore_vanilla_file(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
    canonical_path: &str,
    record: VanillaBackupRecord,
    summary: &mut RestoreSummary,
) {
    let restore_path = match resolve_deploy_path(&record.game_rel_path, &game.path, game_data) {
        Ok(path) => path,
        Err(error) => {
            summary.warnings.push(format!(
                "Could not restore vanilla file '{canonical_path}': invalid deployment path: {error}"
            ));
            return;
        }
    };
    let deploy_path = match find_existing_deploy_path_case_insensitive(
        &record.game_rel_path,
        &game.path,
        game_data,
    ) {
        Ok(Some(path)) => path,
        Ok(None) => restore_path,
        Err(error) => {
            summary.warnings.push(format!(
                "Could not inspect the destination for vanilla file '{canonical_path}': {error:#}"
            ));
            return;
        }
    };
    if deploy_path.exists() {
        match files_match(&record.backup_path, &deploy_path) {
            Ok(true) => finish_restoration(game, tracker, &record, summary).await,
            Ok(false) => summary.warnings.push(format!(
                "Did not overwrite existing file '{}' while restoring '{canonical_path}'; the vanilla backup was retained",
                deploy_path.display()
            )),
            Err(error) => summary.warnings.push(format!(
                "Could not verify existing file '{}' while restoring '{canonical_path}': {error:#}",
                deploy_path.display()
            )),
        }
        return;
    }
    if let Some(parent) = deploy_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        summary.warnings.push(format!(
            "Could not recreate '{}' while restoring '{canonical_path}': {error}",
            parent.display()
        ));
        return;
    }
    if let Err(error) = copy_verified_atomic(&record.backup_path, &deploy_path) {
        summary.warnings.push(format!(
            "Could not restore vanilla file '{canonical_path}': {error:#}"
        ));
        return;
    }
    finish_restoration(game, tracker, &record, summary).await;
}

pub(super) async fn backup_exists(
    game: &Game,
    tracker: &Tracker,
    canonical_path: &str,
) -> Result<bool> {
    let Some(record) = tracker.get_vanilla_backup(&game.id, canonical_path).await? else {
        return Ok(false);
    };
    record
        .backup_path
        .try_exists()
        .with_context(|| {
            format!(
                "Failed to inspect backup '{}'",
                record.backup_path.display()
            )
        })?
        .then(|| hash_file(&record.backup_path))
        .transpose()
        .map(|hash| hash.is_some())
}

pub(super) async fn backup_vanilla_file(
    game: &Game,
    tracker: &Tracker,
    canonical_path: &str,
    original_path: &str,
    source: &Path,
) -> Result<bool> {
    let backup_dir = paths::vanilla_backup_dir(&game.id)
        .context("Failed to resolve the vanilla backup directory")?;
    backup_vanilla_file_in(
        game,
        tracker,
        canonical_path,
        original_path,
        source,
        &backup_dir,
    )
    .await
}

async fn backup_vanilla_file_in(
    game: &Game,
    tracker: &Tracker,
    canonical_path: &str,
    original_path: &str,
    source: &Path,
    backup_dir: &Path,
) -> Result<bool> {
    if let Some(record) = tracker
        .get_vanilla_backup(&game.id, canonical_path)
        .await
        .with_context(|| format!("Failed to query vanilla backup for '{canonical_path}'"))?
        && record.backup_path.try_exists().with_context(|| {
            format!(
                "Failed to inspect vanilla backup '{}'",
                record.backup_path.display()
            )
        })?
    {
        hash_file(&record.backup_path).with_context(|| {
            format!(
                "Failed to verify vanilla backup '{}'",
                record.backup_path.display()
            )
        })?;
        return Ok(false);
    }

    let backup_path = structured_backup_path(backup_dir, original_path)?;
    copy_verified_atomic(source, &backup_path).with_context(|| {
        format!(
            "Failed to back up vanilla file '{}' to '{}'",
            source.display(),
            backup_path.display()
        )
    })?;
    tracker
        .save_vanilla_backup(&game.id, canonical_path, original_path, &backup_path)
        .await
        .with_context(|| format!("Failed to record vanilla backup for '{canonical_path}'"))?;
    Ok(true)
}

async fn finish_restoration(
    game: &Game,
    tracker: &Tracker,
    record: &VanillaBackupRecord,
    summary: &mut RestoreSummary,
) {
    if let Err(error) = tracker
        .delete_vanilla_backup(&game.id, &record.game_rel_path)
        .await
    {
        summary.warnings.push(format!(
            "Could not clear restored backup record '{}': {error}",
            record.game_rel_path
        ));
        return;
    }
    summary.restored += 1;
    if let Err(error) = fs::remove_file(&record.backup_path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        summary.warnings.push(format!(
            "Restored '{}' but could not remove obsolete backup '{}': {error}",
            record.game_rel_path,
            record.backup_path.display()
        ));
    }
}

fn structured_backup_path(backup_dir: &Path, relative: &str) -> Result<PathBuf> {
    let (anchor, rel) = if let Some(rel) = relative.strip_prefix("../") {
        ("root", rel)
    } else if let Some(rel) = relative.strip_prefix(DOCS_PREFIX) {
        ("docs", rel)
    } else {
        ("data", relative)
    };
    let rel_path = Path::new(rel);
    if rel_path.as_os_str().is_empty()
        || rel_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        bail!("invalid vanilla backup path '{relative}'");
    }
    Ok(backup_dir.join(anchor).join(rel_path))
}

fn copy_verified_atomic(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("Vanilla backup or restore destination has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create '{}'", parent.display()))?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("vanilla-file");
    let temporary = parent.join(format!(".{file_name}.deployd-{}", Uuid::new_v4()));
    let result = (|| {
        fs::copy(source, &temporary).with_context(|| {
            format!(
                "copy from '{}' to '{}' failed",
                source.display(),
                temporary.display()
            )
        })?;
        if !files_match(source, &temporary)? {
            bail!("copied file failed SHA-256 verification");
        }
        if destination.exists() {
            if files_match(source, destination)? {
                return Ok(());
            }
            bail!(
                "refusing to overwrite different file '{}'",
                destination.display()
            );
        }
        fs::rename(&temporary, destination).with_context(|| {
            format!(
                "Failed to finalize verified file '{}'",
                destination.display()
            )
        })?;
        Ok(())
    })();
    let cleanup = fs::remove_file(&temporary);
    if result.is_ok()
        && let Err(error) = cleanup
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(error).context("Failed to remove temporary vanilla file");
    }
    result
}

pub(super) fn files_match(left: &Path, right: &Path) -> Result<bool> {
    Ok(hash_file(left)? == hash_file(right)?)
}

fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open '{}' for verification", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed to read '{}'", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;
    use tempfile::tempdir;

    use crate::core::tracker::Tracker;
    use crate::models::game::{Game, GameEngine};

    use super::{
        DOCS_PREFIX, backup_vanilla_file_in, copy_verified_atomic, restore_vanilla_for_paths,
        structured_backup_path,
    };

    fn game(root: &Path) -> Game {
        Game {
            id: "game".to_string(),
            title: "Game".to_string(),
            path: root.to_path_buf(),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: None,
        }
    }

    #[test]
    fn reports_failed_backup_restoration() -> Result<()> {
        let temp = tempdir()?;
        let missing_backup = temp.path().join("missing.backup");
        let destination = temp.path().join("restored.file");

        let error = copy_verified_atomic(&missing_backup, &destination)
            .expect_err("a missing backup must produce a restoration failure");

        assert!(error.to_string().contains("copy from"));
        assert!(!destination.exists());
        Ok(())
    }

    #[test]
    fn separates_backup_anchors_and_ambiguous_paths() -> Result<()> {
        let root = Path::new("/backup");

        assert_eq!(
            structured_backup_path(root, "a/b__c")?,
            Path::new("/backup/data/a/b__c")
        );
        assert_eq!(
            structured_backup_path(root, "a__b/c")?,
            Path::new("/backup/data/a__b/c")
        );
        assert_eq!(
            structured_backup_path(root, "../bin/game.exe")?,
            Path::new("/backup/root/bin/game.exe")
        );
        assert_eq!(
            structured_backup_path(root, "../system/config.ini")?,
            Path::new("/backup/root/system/config.ini")
        );
        assert_eq!(
            structured_backup_path(root, &format!("{DOCS_PREFIX}Settings/config.xml"))?,
            Path::new("/backup/docs/Settings/config.xml")
        );
        Ok(())
    }

    #[tokio::test]
    async fn records_verified_backup_before_original_can_be_removed() -> Result<()> {
        let temp = tempdir()?;
        let game = game(&temp.path().join("game"));
        let source = game.data_dir().join("nested/file.bin");
        let backup_dir = temp.path().join("backups");
        std::fs::create_dir_all(source.parent().expect("source parent"))?;
        std::fs::write(&source, b"vanilla")?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;

        let created = backup_vanilla_file_in(
            &game,
            &tracker,
            "nested/file.bin",
            "Nested/File.bin",
            &source,
            &backup_dir,
        )
        .await?;

        assert!(created);
        assert_eq!(std::fs::read(&source)?, b"vanilla");
        let record = tracker
            .get_vanilla_backup(&game.id, "NESTED/FILE.BIN")
            .await?
            .expect("backup record");
        assert_eq!(record.game_rel_path, "Nested/File.bin");
        assert_eq!(record.backup_path, backup_dir.join("data/Nested/File.bin"));
        assert_eq!(std::fs::read(record.backup_path)?, b"vanilla");
        Ok(())
    }

    #[tokio::test]
    async fn leaves_original_untouched_when_backup_recording_fails() -> Result<()> {
        let temp = tempdir()?;
        let database = temp.path().join("tracker.db");
        let database_url = format!("sqlite://{}?mode=rwc", database.display());
        let tracker = Tracker::open(&database_url).await?.tracker;
        let separate_pool = sqlx::SqlitePool::connect(&database_url).await?;
        sqlx::query(
            "CREATE TRIGGER fail_vanilla_backup_insert
             BEFORE INSERT ON vanilla_backups
             BEGIN
                 SELECT RAISE(FAIL, 'simulated backup record failure');
             END",
        )
        .execute(&separate_pool)
        .await?;
        separate_pool.close().await;

        let game = game(&temp.path().join("game"));
        let source = game.data_dir().join("file.bin");
        let backup_dir = temp.path().join("backups");
        std::fs::create_dir_all(game.data_dir())?;
        std::fs::write(&source, b"vanilla")?;

        let error = backup_vanilla_file_in(
            &game,
            &tracker,
            "file.bin",
            "file.bin",
            &source,
            &backup_dir,
        )
        .await
        .expect_err("database failure must stop replacement");

        assert!(
            error
                .to_string()
                .contains("Failed to record vanilla backup")
        );
        assert_eq!(std::fs::read(&source)?, b"vanilla");
        assert_eq!(std::fs::read(backup_dir.join("data/file.bin"))?, b"vanilla");
        Ok(())
    }

    #[tokio::test]
    async fn keeps_backup_when_restore_destination_differs() -> Result<()> {
        let temp = tempdir()?;
        let game = game(&temp.path().join("game"));
        let destination = game.data_dir().join("file.bin");
        let backup = temp.path().join("backup.bin");
        std::fs::create_dir_all(game.data_dir())?;
        std::fs::write(&destination, b"external")?;
        std::fs::write(&backup, b"vanilla")?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .save_vanilla_backup(&game.id, "file.bin", "file.bin", &backup)
            .await?;

        let result =
            restore_vanilla_for_paths(&game, &tracker, &game.data_dir(), &["file.bin".to_string()])
                .await?;

        assert_eq!(result.restored, 0);
        assert_eq!(std::fs::read(destination)?, b"external");
        assert!(backup.exists());
        assert!(
            tracker
                .get_vanilla_backup(&game.id, "file.bin")
                .await?
                .is_some()
        );
        assert!(result.warnings[0].contains("backup was retained"));
        Ok(())
    }

    #[tokio::test]
    async fn keeps_record_when_backup_payload_is_missing() -> Result<()> {
        let temp = tempdir()?;
        let game = game(&temp.path().join("game"));
        let missing_backup = temp.path().join("missing.bin");
        std::fs::create_dir_all(game.data_dir())?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .save_vanilla_backup(&game.id, "file.bin", "file.bin", &missing_backup)
            .await?;

        let result =
            restore_vanilla_for_paths(&game, &tracker, &game.data_dir(), &["file.bin".to_string()])
                .await?;

        assert_eq!(result.restored, 0);
        assert!(!result.warnings.is_empty());
        assert!(!game.data_dir().join("file.bin").exists());
        assert!(
            tracker
                .get_vanilla_backup(&game.id, "file.bin")
                .await?
                .is_some()
        );
        Ok(())
    }

    #[tokio::test]
    async fn finishes_interrupted_restore_when_contents_match() -> Result<()> {
        let temp = tempdir()?;
        let game = game(&temp.path().join("game"));
        let destination = game.data_dir().join("File.BIN");
        let backup = temp.path().join("backup.bin");
        std::fs::create_dir_all(game.data_dir())?;
        std::fs::write(&destination, b"vanilla")?;
        std::fs::write(&backup, b"vanilla")?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .save_vanilla_backup(&game.id, "file.bin", "file.bin", &backup)
            .await?;

        let result =
            restore_vanilla_for_paths(&game, &tracker, &game.data_dir(), &["file.bin".to_string()])
                .await?;

        assert_eq!(result.restored, 1);
        assert!(result.warnings.is_empty());
        assert!(!backup.exists());
        assert!(
            tracker
                .get_vanilla_backup(&game.id, "file.bin")
                .await?
                .is_none()
        );
        Ok(())
    }
}
