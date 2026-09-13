use anyhow::Result;
use sqlx::SqlitePool;

use crate::core::tracker::Tracker;

const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OBJECT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn fixture() -> Result<SqlitePool> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let pool = tracker.pool;
    sqlx::query("INSERT INTO generation_stores(game_id,store_id,cache_root) VALUES ('game','store','cache'),('other','other-store','other-cache')").execute(&pool).await?;
    sqlx::query("INSERT INTO profiles(id,game_id,name) VALUES ('source','game','Source'),('draft','game','Draft'),('unrelated','other','Other')").execute(&pool).await?;
    sqlx::query("INSERT INTO generations(game_id,id,created_at,originating_profile_id,originating_profile_name,manifest_version,manifest) VALUES ('game',?,'2026-01-01T00:00:00Z','source','Source',1,'{}')").bind(GENERATION).execute(&pool).await?;
    sqlx::query("INSERT INTO generation_objects(game_id,sha256,size) VALUES ('game',?,12)")
        .bind(OBJECT)
        .execute(&pool)
        .await?;
    Ok(pool)
}

async fn reference(pool: &SqlitePool) -> Result<()> {
    sqlx::query("INSERT INTO generation_object_references(game_id,generation_id,sha256) VALUES ('game',?,?)").bind(GENERATION).bind(OBJECT).execute(pool).await?;
    Ok(())
}

async fn journal(pool: &SqlitePool) -> Result<()> {
    sqlx::query("INSERT INTO generation_journals(id,game_id,generation_id,kind,document_version,document) VALUES ('operation','game',?,'deploy',1,'{}')").bind(GENERATION).execute(pool).await?;
    Ok(())
}

async fn delete_generation(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM generations WHERE game_id='game' AND id=?")
        .bind(GENERATION)
        .execute(pool)
        .await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn upgrade_preserves_existing_state_without_inventing_history() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let database = temp.path().join("state.sqlite");
    let url = format!("sqlite://{}?mode=rwc", database.display());
    let legacy = SqlitePool::connect(&url).await?;
    sqlx::query("CREATE TABLE settings(key TEXT PRIMARY KEY,value TEXT)")
        .execute(&legacy)
        .await?;
    sqlx::query("INSERT INTO settings(key,value) VALUES ('last_deployed_profile_game','legacy')")
        .execute(&legacy)
        .await?;
    sqlx::query("CREATE TABLE deployed_files(game_id TEXT NOT NULL,game_rel_lowercase TEXT NOT NULL,game_rel_original TEXT NOT NULL,mod_id TEXT NOT NULL,cache_path TEXT NOT NULL,PRIMARY KEY(game_id,game_rel_lowercase))").execute(&legacy).await?;
    sqlx::query("INSERT INTO deployed_files(game_id,game_rel_lowercase,game_rel_original,mod_id,cache_path) VALUES ('game','old','Old','removed-mod','cache/old')").execute(&legacy).await?;
    legacy.close().await;
    let reopened = Tracker::open(&url).await?.tracker;
    assert_eq!(
        reopened
            .get_setting("last_deployed_profile_game")
            .await?
            .as_deref(),
        Some("legacy")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM deployed_files")
            .fetch_one(&reopened.pool)
            .await?,
        1
    );
    for table in [
        "generations",
        "generation_stores",
        "generation_game_state",
        "generation_journals",
        "generation_activations",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&reopened.pool)
                .await?,
            0
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn history_survives_originating_profile_deletion() -> Result<()> {
    let pool = fixture().await?;
    reference(&pool).await?;
    sqlx::query("DELETE FROM profiles WHERE id='source'")
        .execute(&pool)
        .await?;
    let name: String = sqlx::query_scalar("SELECT originating_profile_name FROM generations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(name, "Source");
    assert!(
        sqlx::query("DELETE FROM generation_objects")
            .execute(&pool)
            .await
            .is_err()
    );
    delete_generation(&pool).await?;
    sqlx::query("DELETE FROM generation_objects")
        .execute(&pool)
        .await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn history_and_object_identities_cannot_be_rewritten() -> Result<()> {
    let pool = fixture().await?;
    for statement in [
        "UPDATE generations SET manifest='{}'",
        "UPDATE generations SET originating_profile_name='Changed'",
        "UPDATE generation_objects SET size=20",
        "UPDATE generation_stores SET store_id='substitute',binding_version=1",
    ] {
        assert!(
            sqlx::query(statement).execute(&pool).await.is_err(),
            "{statement}"
        );
    }
    sqlx::query("UPDATE generation_stores SET cache_root='reauthorized-cache',binding_version=1 WHERE game_id='game'").execute(&pool).await?;
    assert!(
        sqlx::query(
            "UPDATE generation_stores SET cache_root='unversioned-cache' WHERE game_id='game'"
        )
        .execute(&pool)
        .await
        .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn deployed_history_and_live_save_owner_are_protected_independently() -> Result<()> {
    let pool = fixture().await?;
    sqlx::query("INSERT INTO generation_game_state(game_id,deployed_generation_id,deployed_profile_id,live_save_profile_id,live_save_mode) VALUES ('game',?,'source','draft','profile')").bind(GENERATION).execute(&pool).await?;
    assert!(delete_generation(&pool).await.is_err());
    sqlx::query("DELETE FROM profiles WHERE id='source'")
        .execute(&pool)
        .await?;
    assert!(
        sqlx::query("DELETE FROM profiles WHERE id='draft'")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query(
        "UPDATE generation_game_state SET deployed_generation_id=NULL WHERE game_id='game'",
    )
    .execute(&pool)
    .await?;
    delete_generation(&pool).await?;
    assert!(
        sqlx::query("DELETE FROM profiles WHERE id='draft'")
            .execute(&pool)
            .await
            .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn restored_draft_protects_history_until_the_draft_is_deleted() -> Result<()> {
    let pool = fixture().await?;
    sqlx::query("INSERT INTO generation_drafts(profile_id,game_id,generation_id,fingerprint,seed_live_saves) VALUES ('draft','game',?,?,1)").bind(GENERATION).bind(GENERATION).execute(&pool).await?;
    assert!(delete_generation(&pool).await.is_err());
    sqlx::query("DELETE FROM profiles WHERE id='draft'")
        .execute(&pool)
        .await?;
    delete_generation(&pool).await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn references_cannot_cross_game_boundaries() -> Result<()> {
    let pool = fixture().await?;
    assert!(sqlx::query("INSERT INTO generation_drafts(profile_id,game_id,generation_id,fingerprint,seed_live_saves) VALUES ('unrelated','game',?,?,1)").bind(GENERATION).bind(GENERATION).execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO generation_object_references(game_id,generation_id,sha256) VALUES ('other',?,?)").bind(GENERATION).bind(OBJECT).execute(&pool).await.is_err());
    assert!(sqlx::query("INSERT INTO generation_game_state(game_id,live_save_profile_id,live_save_mode) VALUES ('game','unrelated','profile')").execute(&pool).await.is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn recovery_intent_is_immutable_and_commit_decisions_are_monotonic() -> Result<()> {
    let pool = fixture().await?;
    journal(&pool).await?;
    assert!(delete_generation(&pool).await.is_err());
    assert!(
        sqlx::query("UPDATE generation_journals SET document='{\"changed\":true}'")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("UPDATE generation_journals SET committed=1")
        .execute(&pool)
        .await?;
    assert!(
        sqlx::query("UPDATE generation_journals SET committed=0")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM generation_journals")
        .execute(&pool)
        .await?;
    delete_generation(&pool).await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn deletion_cannot_race_references_or_pending_preparation() -> Result<()> {
    let pool = fixture().await?;
    journal(&pool).await?;
    sqlx::query("INSERT INTO generation_pending_objects(operation_id,game_id,sha256) VALUES ('operation','game',?)").bind(OBJECT).execute(&pool).await?;
    assert!(
        sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES ('game',?)")
            .bind(OBJECT)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM generation_journals")
        .execute(&pool)
        .await?;
    reference(&pool).await?;
    assert!(
        sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES ('game',?)")
            .bind(OBJECT)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM generation_object_references")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES ('game',?)")
        .bind(OBJECT)
        .execute(&pool)
        .await?;
    assert!(reference(&pool).await.is_err());
    sqlx::query("DELETE FROM generation_objects")
        .execute(&pool)
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_deletions")
            .fetch_one(&pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn repeated_activation_reuses_history_without_rewriting_it() -> Result<()> {
    let pool = fixture().await?;
    for id in ["first", "second"] {
        sqlx::query("INSERT INTO generation_activations(id,game_id,generation_id,profile_id,created_at,kind) VALUES (?,'game',?,'draft','now','deploy')").bind(id).bind(GENERATION).execute(&pool).await?;
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_activations")
            .fetch_one(&pool)
            .await?,
        2
    );
    assert!(
        sqlx::query("UPDATE generation_activations SET created_at='later'")
            .execute(&pool)
            .await
            .is_err()
    );
    delete_generation(&pool).await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn shared_history_protects_each_games_retained_payload() -> Result<()> {
    let pool = fixture().await?;
    sqlx::query("INSERT INTO generation_shared_revisions(family_id,id,created_at,manifest_version,manifest) VALUES ('family','revision','now',1,'{}')").execute(&pool).await?;
    sqlx::query("INSERT INTO generation_shared_dependencies(game_id,generation_id,family_id,revision_id) VALUES ('game',?,'family','revision')").bind(GENERATION).execute(&pool).await?;
    sqlx::query("INSERT INTO generation_shared_objects(family_id,revision_id,game_id,sha256) VALUES ('family','revision','game',?)").bind(OBJECT).execute(&pool).await?;
    assert!(
        sqlx::query("DELETE FROM generation_shared_revisions")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM generation_objects")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES ('game',?)")
            .bind(OBJECT)
            .execute(&pool)
            .await
            .is_err()
    );
    delete_generation(&pool).await?;
    sqlx::query(
        "INSERT INTO generation_shared_state(family_id,revision_id) VALUES ('family','revision')",
    )
    .execute(&pool)
    .await?;
    assert!(
        sqlx::query("DELETE FROM generation_shared_revisions")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM generation_shared_state")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM generation_shared_revisions")
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM generation_objects")
        .execute(&pool)
        .await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_invalid_content_identities_and_sizes() -> Result<()> {
    let pool = fixture().await?;
    assert!(sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('invalid','game','purge',1.5,'{}')").execute(&pool).await.is_err());
    for hash in ["../outside", "ABCDEF", "", &"g".repeat(64)] {
        assert!(
            sqlx::query("INSERT INTO generation_objects(game_id,sha256,size) VALUES ('game',?,1)")
                .bind(hash)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    for size in ["-1", "1.5", "'invalid'"] {
        assert!(
            sqlx::query(&format!(
                "INSERT INTO generation_objects(game_id,sha256,size) VALUES ('other',?,{size})"
            ))
            .bind(OBJECT)
            .execute(&pool)
            .await
            .is_err()
        );
    }
    assert!(sqlx::query("INSERT INTO generation_stores(game_id,store_id,cache_root) VALUES ('../escape','escape','cache')").execute(&pool).await.is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn replacement_statements_cannot_bypass_immutable_records() -> Result<()> {
    let pool = fixture().await?;
    journal(&pool).await?;
    sqlx::query("UPDATE generation_journals SET committed=1")
        .execute(&pool)
        .await?;
    assert!(sqlx::query("INSERT OR REPLACE INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('replacement','game','purge',1,'{}')").execute(&pool).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT committed FROM generation_journals")
            .fetch_one(&pool)
            .await?,
        1
    );
    assert!(sqlx::query("INSERT OR REPLACE INTO generations(game_id,id,created_at,originating_profile_id,originating_profile_name,manifest_version,manifest) VALUES ('game',?,'later','source','Changed',1,'{}')").bind(GENERATION).execute(&pool).await.is_err());
    assert!(
        sqlx::query(
            "INSERT OR REPLACE INTO generation_objects(game_id,sha256,size) VALUES ('game',?,999)"
        )
        .bind(OBJECT)
        .execute(&pool)
        .await
        .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT size FROM generation_objects")
            .fetch_one(&pool)
            .await?,
        12
    );
    Ok(())
}
