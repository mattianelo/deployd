use anyhow::{Context, Result};

use crate::core::migration_bundle::ExportManifest;
use crate::core::tracker::Tracker;
use crate::models::game::Game;

use super::database::import_database_rows;
use super::filesystem::{CopiedPayload, ImportPaths, cleanup_copied_payload};

pub(super) async fn import_database_transaction(
    tracker: &Tracker,
    export_pool: &sqlx::SqlitePool,
    manifest: &ExportManifest,
    game: &Game,
    import_paths: &ImportPaths,
) -> Result<()> {
    let mut tx = tracker
        .pool
        .begin()
        .await
        .context("Failed to begin import")?;

    import_database_rows(&mut tx, export_pool, manifest, game, import_paths).await?;

    tx.commit()
        .await
        .context("Failed to commit AppImage export import")?;
    Ok(())
}

pub(super) fn rollback_copied_payload_on_error(
    import_result: Result<()>,
    copied_payload: &CopiedPayload,
) -> Result<()> {
    if let Err(error) = import_result {
        cleanup_copied_payload(copied_payload);
        return Err(error);
    }
    Ok(())
}
