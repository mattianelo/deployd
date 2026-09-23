use super::*;
use crate::core::tracker::Tracker;

async fn fixture() -> Result<Tracker> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    for game in ["dragon-age", "other"] {
        sqlx::query("INSERT INTO games(id,hidden) VALUES (?,0)")
            .bind(game)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("INSERT INTO mods(id,game_id,name,archive_hash) VALUES (?,?,? ,?)")
            .bind(game)
            .bind(game)
            .bind(game)
            .bind(game)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("INSERT INTO profiles(id,game_id,name,is_active) VALUES (?,?,'Old',1)")
            .bind(game)
            .bind(game)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("INSERT INTO profile_mods(profile_id,mod_id) VALUES (?,?)")
            .bind(game)
            .bind(game)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("INSERT INTO mod_files(mod_id,game_rel_lowercase,game_rel_original,cache_path) VALUES (?,'file','File','cached')").bind(game).execute(&tracker.pool).await?;
        sqlx::query("INSERT INTO download_entries(id,mod_name,archive_path,archive_hash,status) VALUES (?,?,'archive.zip',?,'installed')")
            .bind(game).bind(game).bind(game).execute(&tracker.pool).await?;
        sqlx::query(
            "INSERT INTO generation_stores(game_id,store_id,cache_root) VALUES (?,?,'cache')",
        )
        .bind(game)
        .bind(game)
        .execute(&tracker.pool)
        .await?;
        sqlx::query("INSERT INTO generations(game_id,id,created_at,originating_profile_id,originating_profile_name,manifest_version,manifest) VALUES (?,?,'now',?,'Old',1,'{}')")
            .bind(game).bind("a".repeat(64)).bind(game).execute(&tracker.pool).await?;
        sqlx::query("INSERT INTO generation_game_state(game_id,deployed_generation_id,deployed_profile_id,live_save_profile_id,live_save_mode) VALUES (?,?,?,?,'profile')")
            .bind(game).bind("a".repeat(64)).bind(game).bind(game).execute(&tracker.pool).await?;
        sqlx::query("INSERT INTO generation_drafts(profile_id,game_id,generation_id,fingerprint,seed_live_saves) VALUES (?,?,?,?,0)")
            .bind(game).bind(game).bind("a".repeat(64)).bind("b".repeat(64)).execute(&tracker.pool).await?;
        tracker.record_deployed_profile(game, game).await?;
    }
    Ok(tracker)
}

// @variants: both
#[tokio::test]
async fn both_removal_choices_reset_management_without_forgetting_archives_or_other_games()
-> Result<()> {
    for delete in [false, true] {
        let tracker = fixture().await?;
        let ids = tracker.remove_managed_game("dragon-age", delete).await?;
        assert_eq!(ids.len(), usize::from(delete));
        for table in [
            "mods",
            "profiles",
            "generations",
            "generation_game_state",
            "generation_drafts",
        ] {
            let games: Vec<String> = sqlx::query_scalar(&format!("SELECT game_id FROM {table}"))
                .fetch_all(&tracker.pool)
                .await?;
            assert_eq!(games, ["other"], "{table}");
        }
        assert!(
            tracker
                .get_setting("last_deployed_profile_dragon-age")
                .await?
                .is_none()
        );
        let downloads: Vec<(String, String, String)> =
            sqlx::query_as("SELECT id,status,archive_path FROM download_entries ORDER BY id")
                .fetch_all(&tracker.pool)
                .await?;
        assert_eq!(
            downloads,
            [
                (
                    "dragon-age".into(),
                    "downloaded".into(),
                    "archive.zip".into()
                ),
                ("other".into(), "installed".into(), "archive.zip".into())
            ]
        );
        tracker
            .upsert_game(
                "dragon-age",
                "Origins",
                std::path::Path::new("/game"),
                "Data",
                "eclipse",
                None,
                false,
            )
            .await?;
        assert!(tracker.list_mods("dragon-age").await?.is_empty());
        assert!(tracker.get_active_profile("dragon-age").await?.is_none());
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn manage_games_unchecking_uses_the_same_reset() -> Result<()> {
    let tracker = fixture().await?;
    tracker
        .persist_game_configs(&[], &["dragon-age".into()])
        .await?;
    assert!(tracker.list_mods("dragon-age").await?.is_empty());
    assert!(tracker.get_active_profile("dragon-age").await?.is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn pending_recovery_blocks_reset_without_partial_deletion() -> Result<()> {
    let tracker = fixture().await?;
    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('pending','dragon-age','deploy',1,'{}')").execute(&tracker.pool).await?;
    assert!(
        tracker
            .remove_managed_game("dragon-age", true)
            .await
            .is_err()
    );
    assert_eq!(tracker.list_mods("dragon-age").await?.len(), 1);
    assert!(tracker.load_hidden_game_ids().await?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_reset_rolls_back_profiles_history_and_download_status() -> Result<()> {
    let tracker = fixture().await?;
    sqlx::query("CREATE TRIGGER reject_reset BEFORE DELETE ON mods BEGIN SELECT RAISE(ABORT,'injected reset failure'); END")
        .execute(&tracker.pool).await?;
    assert!(
        tracker
            .remove_managed_game("dragon-age", false)
            .await
            .is_err()
    );
    assert!(tracker.get_active_profile("dragon-age").await?.is_some());
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM generation_game_state WHERE game_id='dragon-age'")
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(count, 1);
    let status: String =
        sqlx::query_scalar("SELECT status FROM download_entries WHERE id='dragon-age'")
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(status, "installed");
    assert!(tracker.load_hidden_game_ids().await?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn reset_clears_legacy_download_flags_but_preserves_archives_used_by_other_games()
-> Result<()> {
    let tracker = fixture().await?;
    sqlx::query("INSERT INTO download_entries(id,mod_name,game_domain,status) VALUES ('stale','Old mod','dragonage','installed')").execute(&tracker.pool).await?;
    sqlx::query("UPDATE mods SET archive_hash='dragon-age' WHERE game_id='other'")
        .execute(&tracker.pool)
        .await?;
    tracker.remove_managed_game("dragon-age", false).await?;
    let statuses: Vec<(String, String)> = sqlx::query_as(
        "SELECT id,status FROM download_entries WHERE id IN ('dragon-age','stale') ORDER BY id",
    )
    .fetch_all(&tracker.pool)
    .await?;
    assert_eq!(
        statuses,
        [
            ("dragon-age".into(), "installed".into()),
            ("stale".into(), "downloaded".into())
        ]
    );
    Ok(())
}
