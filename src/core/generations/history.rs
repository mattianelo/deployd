use anyhow::{Context, Result, ensure};
use sqlx::{Row, Sqlite, Transaction};

use crate::core::tracker::Tracker;

use super::catalog::{History, durable};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) id: String,
    pub(super) created_at: String,
    pub(super) profile_name: String,
    pub(super) deployed: bool,
    pub(super) modified: bool,
    pub(super) restored_drafts: i64,
    pub(super) recovery_pending: bool,
}

pub(super) async fn list(tracker: &Tracker, game: &str) -> Result<Vec<Entry>> {
    sqlx::query(
        "SELECT g.id,g.created_at,g.originating_profile_name AS profile_name,
         COALESCE(s.deployed_generation_id=g.id,0) AS deployed,
         COALESCE(s.deployed_generation_id=g.id AND s.modified,0) AS modified,
         (SELECT COUNT(*) FROM generation_drafts d WHERE d.game_id=g.game_id AND d.generation_id=g.id) AS restored_drafts,
         EXISTS(SELECT 1 FROM generation_journals j WHERE j.game_id=g.game_id) AS recovery_pending
         FROM generations g LEFT JOIN generation_game_state s ON s.game_id=g.game_id
         WHERE g.game_id=? ORDER BY g.created_at DESC,g.id",
    ).bind(game).fetch_all(&tracker.pool).await?.into_iter().map(|row| Ok(Entry {
        id: row.try_get("id")?, created_at: row.try_get("created_at")?, profile_name: row.try_get("profile_name")?,
        deployed: row.try_get("deployed")?, modified: row.try_get("modified")?,
        restored_drafts: row.try_get("restored_drafts")?, recovery_pending: row.try_get("recovery_pending")?,
    })).collect()
}

pub(super) async fn usage(tracker: &Tracker, game: &str) -> Result<u64> {
    let bytes: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM generation_objects WHERE game_id=?")
            .bind(game)
            .fetch_one(&tracker.pool)
            .await?;
    Ok(bytes.try_into()?)
}

async fn reclaimable(
    tx: &mut Transaction<'_, Sqlite>,
    game: &str,
    generation: &str,
) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT o.sha256,o.size FROM generation_objects o
         JOIN generation_object_references r ON r.game_id=o.game_id AND r.sha256=o.sha256
         WHERE r.game_id=? AND r.generation_id=?
         AND NOT EXISTS(SELECT 1 FROM generation_object_references other WHERE other.game_id=o.game_id AND other.sha256=o.sha256 AND other.generation_id<>r.generation_id)
         AND NOT EXISTS(SELECT 1 FROM generation_shared_objects s WHERE s.game_id=o.game_id AND s.sha256=o.sha256)
         AND NOT EXISTS(SELECT 1 FROM generation_pending_objects p WHERE p.game_id=o.game_id AND p.sha256=o.sha256)",
    ).bind(game).bind(generation).fetch_all(&mut **tx).await?)
}

async fn deletable(tx: &mut Transaction<'_, Sqlite>, game: &str, generation: &str) -> Result<()> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generations WHERE game_id=? AND id=?)")
            .bind(game)
            .bind(generation)
            .fetch_one(&mut **tx)
            .await?;
    ensure!(exists, "Deployment generation is unavailable");
    let deployed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_game_state WHERE game_id=? AND deployed_generation_id=?)")
        .bind(game).bind(generation).fetch_one(&mut **tx).await?;
    ensure!(
        !deployed,
        "This generation is deployed; explicitly deploy another configuration or purge before deleting it"
    );
    let restored: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM generation_drafts WHERE game_id=? AND generation_id=?)",
    )
    .bind(game)
    .bind(generation)
    .fetch_one(&mut **tx)
    .await?;
    ensure!(
        !restored,
        "A restored profile still references this generation; remove that profile or deploy its edited configuration before deleting history"
    );
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
            .bind(game)
            .fetch_one(&mut **tx)
            .await?;
    ensure!(
        !pending,
        "Finish deployment recovery before deleting history"
    );
    Ok(())
}

pub(super) async fn deletion_size(tracker: &Tracker, game: &str, generation: &str) -> Result<u64> {
    let mut tx = tracker.pool.begin().await?;
    deletable(&mut tx, game, generation).await?;
    let objects = reclaimable(&mut tx, game, generation).await?;
    objects.into_iter().try_fold(0_u64, |total, (_, size)| {
        total
            .checked_add(size.try_into()?)
            .context("History size exceeds the supported range")
    })
}

pub(super) async fn delete(history: &History, generation: &str) -> Result<()> {
    let mut tx = durable(&history.tracker).await?;
    deletable(&mut tx, &history.game, generation).await?;
    let objects = reclaimable(&mut tx, &history.game, generation).await?;
    sqlx::query("DELETE FROM generations WHERE game_id=? AND id=?")
        .bind(&history.game)
        .bind(generation)
        .execute(&mut *tx)
        .await?;
    for (hash, _) in objects {
        sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES (?,?)")
            .bind(&history.game)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    history.finish_deletions().await.context("History deletion was recorded, but payload cleanup is incomplete; restore cache access and retry cleanup")
}
