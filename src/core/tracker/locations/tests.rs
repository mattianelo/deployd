use super::*;
use crate::models::game::{Game, GameConfig, GameEngine};

async fn family(tracker: &Tracker) -> Result<LocationRecord> {
    let location = SelectedLocation {
        root: "/run/user/1000/doc/old/MELE".into(),
        host_hint: Some("/games/MELE".into()),
    };
    let configs = (1..=3)
        .map(|index| {
            let relative = PathBuf::from(format!("Game/ME{index}"));
            GameConfig {
                game: Game {
                    id: format!("family-{index}"),
                    title: format!("Family {index}"),
                    path: location.root.join(&relative),
                    data_subdir: "BioGame".to_string(),
                    engine: GameEngine::Bethesda,
                    wine_prefix: None,
                },
                custom: true,
                locations: vec![FolderSelection {
                    role: FolderRole::Game,
                    location: location.clone(),
                    relative,
                }],
            }
        })
        .collect::<Vec<_>>();
    tracker.persist_game_configs(&configs, &[]).await?;
    tracker.folder_location("family-1", FolderRole::Game).await
}

fn replacement() -> SelectedLocation {
    SelectedLocation {
        root: "/run/user/1000/doc/new/MELE".into(),
        host_hint: Some("/games/MELE".into()),
    }
}

// @variants: snap
#[tokio::test]
async fn restores_a_direct_binding_without_changing_path_spelling() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    tracker
        .upsert_game_path("single", Path::new("/run/user/1000/doc/old/Game"))
        .await?;
    let previous = tracker.folder_location("single", FolderRole::Game).await?;
    let selected = SelectedLocation {
        root: "/run/user/1000/doc/new/Game".into(),
        host_hint: None,
    };
    let pending = tracker
        .commit_location_recovery(&previous, &selected, true)
        .await?;
    assert_eq!(
        pending.changes[0].old_path.as_os_str(),
        "/run/user/1000/doc/old/Game"
    );
    assert_eq!(tracker.load_persisted_games().await?[0].path, selected.root);
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn retains_earlier_mappings_when_a_repair_needs_another_grant() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let previous = family(&tracker).await?;
    tracker
        .commit_location_recovery(&previous, &replacement(), true)
        .await?;
    let intermediate = tracker
        .folder_location("family-1", FolderRole::Game)
        .await?;
    let latest = SelectedLocation {
        root: "/run/user/1000/doc/latest/MELE".into(),
        ..replacement()
    };
    let pending = tracker
        .commit_location_recovery(&intermediate, &latest, true)
        .await?;
    assert_eq!(pending.changes.len(), 6);
    assert!(
        pending
            .changes
            .iter()
            .all(|change| change.new_path.starts_with(&latest.root))
    );
    assert!(
        pending
            .changes
            .iter()
            .any(|change| change.old_path.starts_with(&previous.selection.root))
    );
    assert!(
        pending
            .changes
            .iter()
            .any(|change| change.old_path.starts_with(&intermediate.selection.root))
    );
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn rebases_tool_configuration_without_rewriting_sibling_paths() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let previous = family(&tracker).await?;
    let old = previous.selection.root.join("Game/ME1");
    sqlx::query("INSERT INTO tools(id,game_id,name,exe_path,working_dir) VALUES ('tool','family-1','Tool',?,?)")
        .bind(old.join("Tools/editor.exe").to_string_lossy().as_ref()).bind(old.join("Tools").to_string_lossy().as_ref()).execute(&tracker.pool).await?;
    sqlx::query(
        "INSERT INTO tools(id,game_id,name,exe_path) VALUES ('other','family-1','Other',?)",
    )
    .bind(format!("{}-other/editor.exe", old.display()))
    .execute(&tracker.pool)
    .await?;
    tracker
        .commit_location_recovery(&previous, &replacement(), true)
        .await?;
    let (exe, working): (String, String) =
        sqlx::query_as("SELECT exe_path,working_dir FROM tools WHERE id='tool'")
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(
        Path::new(&exe),
        replacement().root.join("Game/ME1/Tools/editor.exe")
    );
    assert_eq!(
        Path::new(&working),
        replacement().root.join("Game/ME1/Tools")
    );
    let other: String = sqlx::query_scalar("SELECT exe_path FROM tools WHERE id='other'")
        .fetch_one(&tracker.pool)
        .await?;
    assert_eq!(other, format!("{}-other/editor.exe", old.display()));
    Ok(())
}

// @variants: both
#[tokio::test]
async fn excludes_location_permissions_and_pending_repairs_from_exports() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let previous = family(&tracker).await?;
    tracker
        .commit_location_recovery(&previous, &replacement(), true)
        .await?;
    let game = crate::core::location_recovery::persisted_game(
        tracker.load_persisted_games().await?.remove(0),
    );
    crate::core::migration_export::prune_export_database(&tracker.pool, &game).await?;
    for table in ["location_repairs", "game_locations", "folder_locations"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(count, 0);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preserves_shared_relative_bindings_during_routine_saves() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let root = family(&tracker).await?;
    assert_eq!(root.bindings.len(), 3);
    tracker
        .upsert_game(
            "family-1",
            "Renamed",
            &root.selection.root.join("Game/ME1"),
            "BioGame",
            "bethesda",
            None,
            true,
        )
        .await?;
    let saved = tracker
        .folder_location("family-1", FolderRole::Game)
        .await?;
    assert_eq!(saved.id, root.id);
    assert_eq!(saved.selection, root.selection);
    assert_eq!(saved.bindings[0].relative, Path::new("Game/ME1"));
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn commits_all_family_paths_with_a_durable_repair_record() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let root = family(&tracker).await?;
    let replacement = replacement();
    let repair = tracker
        .commit_location_recovery(&root, &replacement, false)
        .await?;
    let games = tracker.load_persisted_games().await?;
    assert_eq!(games.len(), 3);
    assert!(
        games
            .iter()
            .all(|game| game.path.starts_with(&replacement.root))
    );
    assert_eq!(repair.changes.len(), 3);
    assert_eq!(
        tracker.pending_location_repairs().await?[0].changes,
        repair.changes
    );
    assert!(tracker.ensure_location_ready("family-2").await.is_err());
    tracker.finish_location_repair(&repair).await?;
    tracker.ensure_location_ready("family-2").await?;
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn rolls_back_every_path_when_one_family_update_fails() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let root = family(&tracker).await?;
    sqlx::query("CREATE TRIGGER reject_third_path BEFORE UPDATE OF path ON games WHEN NEW.id='family-3' BEGIN SELECT RAISE(ABORT,'injected failure'); END").execute(&tracker.pool).await?;
    assert!(
        tracker
            .commit_location_recovery(&root, &replacement(), false)
            .await
            .is_err()
    );
    assert!(
        tracker
            .load_persisted_games()
            .await?
            .iter()
            .all(|game| game.path.starts_with(&root.selection.root))
    );
    assert_eq!(
        tracker
            .folder_location("family-1", FolderRole::Game)
            .await?
            .selection,
        root.selection
    );
    assert!(tracker.pending_location_repairs().await?.is_empty());
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn retains_old_bindings_when_the_database_commit_fails() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let root = family(&tracker).await?;
    sqlx::query("CREATE TABLE commit_failure (location INTEGER REFERENCES folder_locations(id) DEFERRABLE INITIALLY DEFERRED)").execute(&tracker.pool).await?;
    sqlx::query("CREATE TRIGGER reject_recovery_commit AFTER INSERT ON location_repairs BEGIN INSERT INTO commit_failure VALUES (-123); END").execute(&tracker.pool).await?;
    assert!(
        tracker
            .commit_location_recovery(&root, &replacement(), true)
            .await
            .is_err()
    );
    assert!(tracker.pending_location_repairs().await?.is_empty());
    assert_eq!(
        tracker
            .folder_location("family-1", FolderRole::Game)
            .await?
            .selection,
        root.selection
    );
    assert!(
        tracker
            .load_persisted_games()
            .await?
            .iter()
            .all(|game| game.path.starts_with(&root.selection.root))
    );
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn preserves_configuration_on_a_unique_game_path_conflict() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let root = family(&tracker).await?;
    tracker
        .upsert_game(
            "other",
            "Other",
            &replacement().root.join("Game/ME2"),
            "Data",
            "bethesda",
            None,
            true,
        )
        .await?;
    assert!(
        tracker
            .commit_location_recovery(&root, &replacement(), false)
            .await
            .is_err()
    );
    assert_eq!(
        tracker
            .folder_location("family-1", FolderRole::Game)
            .await?
            .selection,
        root.selection
    );
    assert!(tracker.pending_location_repairs().await?.is_empty());
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn rejects_stale_picker_results_and_unknown_identity_without_confirmation() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let root = family(&tracker).await?;
    let unknown = SelectedLocation {
        host_hint: None,
        ..replacement()
    };
    assert!(
        tracker
            .commit_location_recovery(&root, &unknown, false)
            .await
            .is_err()
    );
    tracker
        .upsert_game_path("family-2", Path::new("/different/game"))
        .await?;
    assert!(
        tracker
            .commit_location_recovery(&root, &replacement(), true)
            .await
            .is_err()
    );
    assert!(tracker.pending_location_repairs().await?.is_empty());
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn keeps_unfinished_repairs_after_reopening_the_database() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let url = format!(
        "sqlite://{}?mode=rwc",
        temp.path().join("tracker.db").display()
    );
    let tracker = Tracker::open(&url).await?.tracker;
    let root = family(&tracker).await?;
    let pending = tracker
        .commit_location_recovery(&root, &replacement(), true)
        .await?;
    tracker.pool.close().await;
    let tracker = Tracker::open(&url).await?.tracker;
    assert_eq!(
        tracker.pending_location_repairs().await?[0].changes,
        pending.changes
    );
    assert!(tracker.ensure_location_ready("family-1").await.is_err());
    assert!(
        tracker
            .upsert_game_path("family-1", Path::new("/unrelated"))
            .await
            .is_err()
    );
    tracker.finish_location_repair(&pending).await?;
    tracker.ensure_location_ready("family-1").await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn backfills_legacy_paths_without_inventing_host_hints() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    sqlx::query("INSERT INTO games(id,title,path,wine_prefix) VALUES ('legacy','Legacy','/run/user/1000/doc/old/Game','/run/user/1000/doc/old/Prefix')").execute(&tracker.pool).await?;
    migrate(&tracker.pool).await?;
    for role in [FolderRole::Game, FolderRole::Prefix] {
        let root = tracker.folder_location("legacy", role).await?;
        assert!(root.selection.host_hint.is_none());
        assert!(root.bindings[0].relative.as_os_str().is_empty());
    }
    tracker
        .upsert_game(
            "legacy",
            "Legacy",
            Path::new("/new/Game"),
            "Data",
            "bethesda",
            None,
            true,
        )
        .await?;
    assert!(
        tracker
            .folder_location("legacy", FolderRole::Prefix)
            .await
            .is_err()
    );
    assert_eq!(
        tracker
            .folder_location("legacy", FolderRole::Game)
            .await?
            .selection
            .root,
        Path::new("/new/Game")
    );
    Ok(())
}
