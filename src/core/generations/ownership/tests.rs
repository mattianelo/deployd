use super::*;

use crate::core::save_manager::SaveSetId;

async fn fixture() -> Result<(Tracker, String)> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let profile = tracker.create_profile("game", "Selected").await?;
    sqlx::query("UPDATE profiles SET is_active=1,save_mode='profile' WHERE id=?")
        .bind(&profile)
        .execute(&tracker.pool)
        .await?;
    Ok((tracker, profile))
}

async fn read(tracker: &Tracker) -> Result<Option<State>> {
    let mut tx = durable(tracker).await?;
    let state = state::read(&mut tx, "game").await?;
    tx.rollback().await?;
    Ok(state)
}

// @variants: both
#[tokio::test]
async fn imports_selected_save_owner_separately_from_legacy_deployment() -> Result<()> {
    let (tracker, selected) = fixture().await?;
    let deployed = tracker.create_profile("game", "Deployed").await?;
    tracker.record_deployed_profile("game", &deployed).await?;
    sqlx::query("INSERT INTO deployed_files(game_id,game_rel_lowercase,game_rel_original,mod_id,cache_path) VALUES ('game','file.txt','File.txt','removed-mod','legacy-cache')")
        .execute(&tracker.pool).await?;
    let state = import(&tracker, "game").await?;
    assert_eq!(state.generation, None);
    assert_eq!(state.profile.as_deref(), Some(deployed.as_str()));
    assert_eq!(
        state.saves,
        SaveSetId::Profile {
            game_id: "game".into(),
            profile_id: selected.clone()
        }
    );
    assert_eq!(read(&tracker).await?, Some(state));
    assert_eq!(
        tracker.get_active_profile("game").await?.map(|p| p.id),
        Some(selected)
    );
    for table in [
        "generations",
        "generation_activations",
        "generation_objects",
        "generation_stores",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(count, 0, "Initialization populated {table}");
    }
    let files: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM deployed_files WHERE game_id='game' AND cache_path='legacy-cache'",
    )
    .fetch_one(&tracker.pool)
    .await?;
    assert_eq!(files, 1);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn existing_ownership_survives_draft_and_legacy_setting_changes() -> Result<()> {
    let (tracker, _) = fixture().await?;
    let state = import(&tracker, "game").await?;
    sqlx::query("UPDATE profiles SET is_active=0,save_mode='global'")
        .execute(&tracker.pool)
        .await?;
    let draft = tracker.create_profile("game", "New draft").await?;
    sqlx::query("UPDATE profiles SET is_active=1 WHERE id=?")
        .bind(&draft)
        .execute(&tracker.pool)
        .await?;
    tracker.record_deployed_profile("game", &draft).await?;
    assert_eq!(import(&tracker, "game").await?, state);
    assert_eq!(read(&tracker).await?, Some(state));
    Ok(())
}

// @variants: both
#[tokio::test]
async fn global_ownership_does_not_adopt_stale_or_other_game_deployment_markers() -> Result<()> {
    let (tracker, _) = fixture().await?;
    sqlx::query("UPDATE profiles SET save_mode='global'")
        .execute(&tracker.pool)
        .await?;
    let other = tracker.create_profile("other-game", "Other").await?;
    for marker in ["deleted-profile", other.as_str()] {
        tracker.record_deployed_profile("game", marker).await?;
        let state = import(&tracker, "game").await?;
        assert_eq!(state.generation, None);
        assert_eq!(state.profile, None);
        assert_eq!(
            state.saves,
            SaveSetId::Global {
                game_id: "game".into()
            }
        );
        assert_eq!(
            tracker
                .get_setting("last_deployed_profile_game")
                .await?
                .as_deref(),
            Some(marker)
        );
        sqlx::query("DELETE FROM generation_game_state")
            .execute(&tracker.pool)
            .await?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn ambiguous_or_unsupported_selection_does_not_create_ownership() -> Result<()> {
    let (tracker, selected) = fixture().await?;
    sqlx::query("UPDATE profiles SET is_active=0")
        .execute(&tracker.pool)
        .await?;
    assert!(import(&tracker, "game").await.is_err());
    assert_eq!(read(&tracker).await?, None);
    tracker.create_profile("game", "Second").await?;
    sqlx::query("UPDATE profiles SET is_active=1")
        .execute(&tracker.pool)
        .await?;
    assert!(import(&tracker, "game").await.is_err());
    assert_eq!(read(&tracker).await?, None);
    sqlx::query("UPDATE profiles SET is_active=(id=?),save_mode='unsupported'")
        .bind(selected)
        .execute(&tracker.pool)
        .await?;
    assert!(import(&tracker, "game").await.is_err());
    assert_eq!(read(&tracker).await?, None);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_ownership_commit_can_be_retried_without_losing_legacy_state() -> Result<()> {
    let (tracker, selected) = fixture().await?;
    sqlx::query("CREATE TRIGGER reject_owner BEFORE INSERT ON generation_game_state BEGIN SELECT RAISE(ABORT,'injected failure'); END")
        .execute(&tracker.pool).await?;
    assert!(import(&tracker, "game").await.is_err());
    assert_eq!(read(&tracker).await?, None);
    assert_eq!(
        tracker.get_active_profile("game").await?.map(|p| p.id),
        Some(selected)
    );
    sqlx::query("DROP TRIGGER reject_owner")
        .execute(&tracker.pool)
        .await?;
    let imported = import(&tracker, "game").await?;
    assert_eq!(read(&tracker).await?, Some(imported));
    Ok(())
}

// @variants: both
#[tokio::test]
async fn pending_recovery_blocks_initialization_without_replacing_existing_ownership() -> Result<()>
{
    let (tracker, _) = fixture().await?;
    for initialized in [false, true] {
        let previous = if initialized {
            Some(import(&tracker, "game").await?)
        } else {
            None
        };
        sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('pending','game','deploy',1,'{}')")
            .execute(&tracker.pool).await?;
        assert!(import(&tracker, "game").await.is_err());
        assert_eq!(read(&tracker).await?, previous);
        let pending: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM generation_journals WHERE id='pending'")
                .fetch_one(&tracker.pool)
                .await?;
        assert_eq!(pending, 1);
        sqlx::query("DELETE FROM generation_journals WHERE id='pending'")
            .execute(&tracker.pool)
            .await?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn imported_ownership_survives_restart_without_following_the_selected_draft() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}?mode=rwc",
        temp.path().join("tracker.db").display()
    );
    let tracker = Tracker::open(&url).await?.tracker;
    let owner = tracker.create_profile("game", "Owner").await?;
    sqlx::query("UPDATE profiles SET is_active=1,save_mode='profile' WHERE id=?")
        .bind(&owner)
        .execute(&tracker.pool)
        .await?;
    let imported = import(&tracker, "game").await?;
    let draft = tracker.create_profile("game", "Draft").await?;
    sqlx::query("UPDATE profiles SET is_active=(id=?)")
        .bind(&draft)
        .execute(&tracker.pool)
        .await?;
    tracker.pool.close().await;
    drop(tracker);
    let reopened = Tracker::open(&url).await?.tracker;
    assert_eq!(import(&reopened, "game").await?, imported);
    assert_eq!(
        reopened.get_active_profile("game").await?.map(|p| p.id),
        Some(draft)
    );
    assert_eq!(imported.saves.profile_id(), Some(owner.as_str()));
    Ok(())
}
