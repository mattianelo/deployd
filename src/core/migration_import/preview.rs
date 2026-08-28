use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::core::migration_bundle::ExportManifest;
use crate::core::tracker::Tracker;

use super::bundle::{BundleFileCounts, extract_preview_bundle};
use super::{open_sqlite_pool, sqlite_url_read_only};

#[derive(Debug, Clone)]
pub struct PreviewImportRequest {
    pub bundle_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreviewImportResult {
    pub bundle_path: PathBuf,
    pub manifest: ExportManifest,
    pub counts: PreviewCounts,
    pub conflict: PreviewConflict,
    pub validation_items: Vec<ValidationItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewCounts {
    pub mods: i64,
    pub plugins: i64,
    pub profiles: i64,
    pub tools: i64,
    pub downloads: i64,
    pub cache_files: usize,
    pub vanilla_backups: usize,
    pub save_snapshots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewConflict {
    NewGame,
    ExistingGame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationItem {
    NeedsGameFolderConfirmation,
    NeedsWinePrefixConfirmation,
    ToolsNeedSnapRuntimeRebind,
}

pub async fn preview_import_bundle(
    tracker: &Tracker,
    request: PreviewImportRequest,
) -> Result<PreviewImportResult> {
    let extracted = tokio::task::spawn_blocking({
        let bundle_path = request.bundle_path.clone();
        move || extract_preview_bundle(&bundle_path)
    })
    .await
    .context("Import preview task failed")??;

    let db_url = sqlite_url_read_only(&extracted.export_db);
    let pool = open_sqlite_pool(&db_url).await?;
    let counts = read_preview_counts(&pool, extracted.bundle_files).await?;
    pool.close().await;

    let existing_state = tracker_game_state(tracker, &extracted.manifest.game_id).await?;

    let mut warnings = extracted.manifest.warnings.clone();
    if existing_state == ExistingGameState::Active {
        warnings.push(
            "This game is already managed in the Snap; a later import phase will skip it by default."
                .to_string(),
        );
    } else if existing_state == ExistingGameState::Hidden {
        warnings.push(
            "This game was previously stopped in the Snap; import will replace that hidden state."
                .to_string(),
        );
    }

    let mut validation_items = vec![
        ValidationItem::NeedsGameFolderConfirmation,
        ValidationItem::NeedsWinePrefixConfirmation,
    ];
    if counts.tools > 0 {
        validation_items.push(ValidationItem::ToolsNeedSnapRuntimeRebind);
    }

    Ok(PreviewImportResult {
        bundle_path: request.bundle_path,
        manifest: extracted.manifest,
        counts,
        conflict: if existing_state == ExistingGameState::Active {
            PreviewConflict::ExistingGame
        } else {
            PreviewConflict::NewGame
        },
        validation_items,
        warnings,
    })
}

pub(super) async fn read_preview_counts(
    pool: &sqlx::SqlitePool,
    bundle_files: BundleFileCounts,
) -> Result<PreviewCounts> {
    Ok(PreviewCounts {
        mods: count_rows(pool, "mods").await?,
        plugins: count_rows(pool, "plugins").await?,
        profiles: count_rows(pool, "profiles").await?,
        tools: count_rows(pool, "tools").await?,
        downloads: count_rows(pool, "download_entries").await?,
        cache_files: bundle_files.cache_files,
        vanilla_backups: bundle_files.vanilla_backups,
        save_snapshots: bundle_files.save_snapshots,
    })
}

async fn count_rows(pool: &sqlx::SqlitePool, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    sqlx::query_scalar(&sql)
        .fetch_one(pool)
        .await
        .with_context(|| format!("Failed to count {table}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExistingGameState {
    Absent,
    Active,
    Hidden,
}

pub(super) async fn tracker_game_state(
    tracker: &Tracker,
    game_id: &str,
) -> Result<ExistingGameState> {
    let hidden: Option<Option<i32>> = sqlx::query_scalar("SELECT hidden FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&tracker.pool)
        .await
        .context("Failed to check existing Snap game")?;
    Ok(match hidden {
        None => ExistingGameState::Absent,
        Some(Some(1)) => ExistingGameState::Hidden,
        Some(_) => ExistingGameState::Active,
    })
}
