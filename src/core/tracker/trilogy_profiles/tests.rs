use super::*;

async fn fixture() -> Result<Tracker> {
    fixture_at("sqlite::memory:").await
}

async fn fixture_at(database: &str) -> Result<Tracker> {
    use crate::models::game::{Game, GameConfig, GameEngine};
    use crate::utils::location::{FolderRole, FolderSelection, SelectedLocation};
    let tracker = Tracker::open(database).await?.tracker;
    let configs = GAMES
        .iter()
        .enumerate()
        .map(|(i, id)| GameConfig {
            game: Game {
                id: (*id).into(),
                title: format!("LE{}", i + 1),
                path: format!("/trilogy/Game/ME{}", i + 1).into(),
                data_subdir: "BioGame".into(),
                engine: GameEngine::MassEffect,
                wine_prefix: None,
            },
            custom: true,
            locations: vec![FolderSelection {
                role: FolderRole::Game,
                location: SelectedLocation {
                    root: "/trilogy".into(),
                    host_hint: None,
                },
                relative: format!("Game/ME{}", i + 1).into(),
            }],
        })
        .collect::<Vec<_>>();
    tracker.persist_game_configs(&configs, &[]).await?;
    for game in GAMES {
        sqlx::query("INSERT INTO mods(id,game_id,name,enabled,priority) VALUES (?,?,?,1,7)")
            .bind(format!("mod-{game}"))
            .bind(game)
            .bind(game)
            .execute(&tracker.pool)
            .await?;
    }
    Ok(tracker)
}

// @variants: both
#[tokio::test]
async fn fresh_trilogy_shares_profile_selection_but_preserves_separate_mod_lists() -> Result<()> {
    let tracker = fixture().await?;
    let first = tracker.ensure_default_profile(GAMES[0]).await?;
    let parts = tracker.profile_parts(&first.id).await?;
    assert_eq!(parts.len(), 3);
    let clean = tracker.create_clean_profile(GAMES[1], "Clean").await?;
    tracker.switch_profile(GAMES[1], &clean).await?;
    for game in GAMES {
        assert_eq!(
            tracker.get_active_profile(game).await?.unwrap().name,
            "Clean"
        );
        let enabled: bool = sqlx::query_scalar("SELECT enabled FROM mods WHERE game_id=?")
            .bind(game)
            .fetch_one(&tracker.pool)
            .await?;
        assert!(!enabled);
    }
    tracker.switch_profile(GAMES[0], &first.id).await?;
    for (game, id) in parts {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM profile_mods pm JOIN mods m ON m.id=pm.mod_id WHERE pm.profile_id=? AND m.game_id<>?").bind(&id).bind(&game).fetch_one(&tracker.pool).await?;
        assert_eq!(count, 0);
        let enabled: bool = sqlx::query_scalar("SELECT enabled FROM mods WHERE game_id=?")
            .bind(game)
            .fetch_one(&tracker.pool)
            .await?;
        assert!(enabled);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn grouping_preserves_ids_unmatched_profiles_and_live_save_owners() -> Result<()> {
    let tracker = fixture().await?;
    let mut ids = Vec::new();
    for game in GAMES {
        let id = tracker.create_profile(game, &format!("Old {game}")).await?;
        tracker.switch_profile(game, &id).await?;
        tracker.save_to_profile(&id, game).await?;
        sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode,live_save_profile_id,modified) VALUES (?,'profile',?,0)").bind(game).bind(&id).execute(&tracker.pool).await?;
        ids.push(id);
    }
    let unmatched = tracker.create_profile(GAMES[1], "Unmatched").await?;
    let mapping = Mapping {
        name: "Shepard".into(),
        profiles: ids.clone().try_into().unwrap(),
        mode: SaveMode::ProfileSpecific,
    };
    tracker.group_trilogy_profiles(GAMES[0], &mapping).await?;
    tracker.rename_profile(&ids[1], "Renamed").await?;
    tracker
        .set_profile_save_mode(&ids[2], SaveMode::Global)
        .await?;
    for (game, id) in GAMES.iter().zip(&ids) {
        let active = tracker.get_active_profile(game).await?.unwrap();
        assert_eq!(&active.id, id);
        assert_eq!(active.name, "Renamed");
        assert_eq!(active.save_mode, SaveMode::Global);
        assert!(tracker.profile_has_live_saves(id).await?);
    }
    assert!(
        tracker.trilogy_candidates(GAMES[1]).await?[1]
            .profiles
            .iter()
            .any(|p| p.id == unmatched)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn grouping_and_selection_roll_back_on_member_failure() -> Result<()> {
    let tracker = fixture().await?;
    let first = tracker.ensure_default_profile(GAMES[0]).await?;
    let next = tracker.create_clean_profile(GAMES[0], "Next").await?;
    sqlx::query("CREATE TRIGGER fail_select BEFORE UPDATE OF is_active ON profiles WHEN NEW.game_id='mass-effect-le3' BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&tracker.pool).await?;
    assert!(tracker.switch_profile(GAMES[0], &next).await.is_err());
    for (game, id) in tracker.profile_parts(&first.id).await? {
        assert_eq!(tracker.get_active_profile(&game).await?.unwrap().id, id);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_cross_game_mapping_without_changing_any_profile() -> Result<()> {
    let tracker = fixture().await?;
    let mut ids = Vec::new();
    for game in GAMES {
        ids.push(tracker.create_profile(game, "Old").await?);
    }
    let mapping = Mapping {
        name: "New".into(),
        profiles: [ids[0].clone(), ids[2].clone(), ids[1].clone()],
        mode: SaveMode::ProfileSpecific,
    };
    assert!(
        tracker
            .group_trilogy_profiles(GAMES[0], &mapping)
            .await
            .is_err()
    );
    for game in GAMES {
        assert_eq!(tracker.list_profiles(game).await?[0].name, "Old");
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn clone_copies_all_three_snapshots_and_delete_protects_each_live_bank() -> Result<()> {
    let tracker = fixture().await?;
    let source = tracker.ensure_default_profile(GAMES[0]).await?;
    tracker
        .set_profile_save_mode(&source.id, SaveMode::ProfileSpecific)
        .await?;
    let cloned = tracker.clone_profile(&source.id, "Copy", GAMES[0]).await?;
    let parts = tracker.profile_parts(&cloned).await?;
    assert_eq!(parts.len(), 3);
    for (_, id) in &parts {
        let enabled: bool =
            sqlx::query_scalar("SELECT enabled FROM profile_mods WHERE profile_id=?")
                .bind(id)
                .fetch_one(&tracker.pool)
                .await?;
        assert!(enabled);
    }
    sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode,live_save_profile_id,modified) VALUES (?,'profile',?,0)").bind(GAMES[2]).bind(&parts[2].1).execute(&tracker.pool).await?;
    assert!(tracker.delete_profile(&cloned).await.is_err());
    assert_eq!(tracker.profile_parts(&cloned).await?.len(), 3);
    sqlx::query("DELETE FROM generation_game_state")
        .execute(&tracker.pool)
        .await?;
    tracker.delete_profile(&cloned).await?;
    assert!(tracker.profile_parts(&cloned).await?.is_empty());
    assert_eq!(tracker.profile_parts(&source.id).await?.len(), 3);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn non_mele_profiles_remain_independent() -> Result<()> {
    let tracker = fixture().await?;
    let first = tracker.create_clean_profile("skyrim", "First").await?;
    tracker.switch_profile("skyrim", &first).await?;
    assert_eq!(
        tracker.profile_parts(&first).await?,
        vec![("skyrim".into(), first.clone())]
    );
    tracker
        .set_profile_save_mode(&first, SaveMode::ProfileSpecific)
        .await?;
    assert!(tracker.get_active_profile(GAMES[0]).await?.is_none());
    Ok(())
}

async fn with_competing_writer<T>(
    tracker: &Tracker,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let mut writer = tracker.pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE mods SET priority=priority+1")
        .execute(&mut *writer)
        .await?;
    tokio::pin!(operation);
    let early = tokio::time::timeout(std::time::Duration::from_millis(100), &mut operation).await;
    writer.commit().await?;
    assert!(
        early.is_err(),
        "Profile mutation must wait for the competing writer"
    );
    operation.await
}

// @variants: both
#[tokio::test]
async fn trilogy_profile_mutations_wait_for_competing_writes() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let database = format!(
        "sqlite://{}?mode=rwc",
        directory.path().join("tracker.db").display()
    );
    let tracker = fixture_at(&database).await?;
    let initial = tracker.ensure_default_profile(GAMES[0]).await?;
    let clean =
        with_competing_writer(&tracker, tracker.create_clean_profile(GAMES[0], "Clean")).await?;
    with_competing_writer(&tracker, tracker.switch_profile(GAMES[0], &clean)).await?;
    for game in GAMES {
        assert_eq!(
            tracker.get_active_profile(game).await?.unwrap().name,
            "Clean"
        );
        let enabled: bool = sqlx::query_scalar("SELECT enabled FROM mods WHERE game_id=?")
            .bind(game)
            .fetch_one(&tracker.pool)
            .await?;
        assert!(!enabled);
    }
    let cloned =
        with_competing_writer(&tracker, tracker.clone_profile(&clean, "Copy", GAMES[0])).await?;
    assert_eq!(tracker.profile_parts(&cloned).await?.len(), 3);
    with_competing_writer(&tracker, tracker.delete_profile(&cloned)).await?;
    assert!(tracker.profile_parts(&cloned).await?.is_empty());
    let mut ids = Vec::new();
    for game in GAMES {
        ids.push(tracker.create_profile(game, "Ungrouped").await?);
    }
    let mapping = Mapping {
        name: "Grouped".into(),
        profiles: ids.try_into().unwrap(),
        mode: SaveMode::Global,
    };
    let grouped =
        with_competing_writer(&tracker, tracker.group_trilogy_profiles(GAMES[0], &mapping)).await?;
    assert_eq!(tracker.profile_parts(&grouped).await?.len(), 3);
    assert_eq!(tracker.profile_parts(&initial.id).await?.len(), 3);
    tracker.pool.close().await;
    Ok(())
}
