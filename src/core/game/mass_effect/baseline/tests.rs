use std::os::unix::fs::symlink;

use tempfile::tempdir;

use super::*;

fn game(root: &Path, number: usize) -> Result<Game> {
    let root = root.join(format!("ME{number}"));
    fs::create_dir_all(root.join("BioGame/CookedPCConsole"))?;
    fs::create_dir_all(root.join("Binaries/Win64"))?;
    fs::write(
        root.join(format!("Binaries/Win64/MassEffect{number}.exe")),
        b"executable",
    )?;
    fs::write(
        root.join("BioGame/CookedPCConsole/Engine.pcc"),
        b"user supplied originals",
    )?;
    Ok(Game {
        id: format!("mass-effect-le{number}"),
        title: format!("LE{number}"),
        path: root,
        data_subdir: "BioGame".into(),
        engine: GameEngine::MassEffect,
        wine_prefix: None,
    })
}

fn config(game: Game) -> GameConfig {
    GameConfig {
        game,
        custom: true,
        locations: Vec::new(),
    }
}

// @variants: both
#[test]
fn assumes_supplied_content_is_clean_and_hashes_only_the_game_root() -> Result<()> {
    let temp = tempdir()?;
    let game = game(temp.path(), 1)?;
    fs::write(temp.path().join("unrelated-save.pcsav"), b"save")?;
    fs::write(
        game.path.join("BioGame/UnknownMod.pcc"),
        b"user responsibility",
    )?;
    fs::write(game.path.join("BioGame/empty"), b"")?;
    let baseline = scan(&game, &AtomicBool::new(false))?;
    assert_eq!(baseline.files.len(), 4);
    assert!(
        baseline
            .files
            .iter()
            .any(|file| file.relative == "BioGame/UnknownMod.pcc")
    );
    assert!(baseline.files.iter().any(|file| file.size == 0));
    assert!(
        baseline
            .files
            .iter()
            .all(|file| !file.relative.contains("pcsav"))
    );
    let again = scan(&game, &AtomicBool::new(false))?;
    assert_eq!(baseline.sha256, again.sha256);
    assert_eq!(fs::read(temp.path().join("unrelated-save.pcsav"))?, b"save");
    Ok(())
}

// @variants: both
#[test]
fn content_changes_with_the_same_size_change_the_baseline_identity() -> Result<()> {
    let temp = tempdir()?;
    let game = game(temp.path(), 1)?;
    let before = scan(&game, &AtomicBool::new(false))?;
    fs::write(
        game.path.join("Binaries/Win64/MassEffect1.exe"),
        b"EXECUTABLE",
    )?;
    let after = scan(&game, &AtomicBool::new(false))?;
    assert_ne!(before.sha256, after.sha256);
    Ok(())
}

// @variants: both
#[test]
fn rejects_linked_files_directories_and_case_collisions() -> Result<()> {
    for directory_link in [false, true] {
        let temp = tempdir()?;
        let game = game(temp.path(), 1)?;
        let outside = temp.path().join("outside");
        if directory_link {
            fs::create_dir(&outside)?;
        } else {
            fs::write(&outside, b"outside")?;
        }
        symlink(&outside, game.path.join("BioGame/escape"))?;
        assert!(scan(&game, &AtomicBool::new(false)).is_err());
    }
    let temp = tempdir()?;
    let game = game(temp.path(), 1)?;
    fs::write(
        game.path.join("BioGame/CookedPCConsole/engine.PCC"),
        b"collision",
    )?;
    assert!(scan(&game, &AtomicBool::new(false)).is_err());
    Ok(())
}

// @variants: both
#[test]
fn rejects_cancellation_missing_roots_and_wrong_game_identity() -> Result<()> {
    let temp = tempdir()?;
    let mut game = game(temp.path(), 1)?;
    assert!(scan(&game, &AtomicBool::new(true)).is_err());
    game.id = "mass-effect-le2".into();
    assert!(scan(&game, &AtomicBool::new(false)).is_err());
    game.id = "skyrimse".into();
    assert!(scan(&game, &AtomicBool::new(false)).is_err());
    game.path = temp.path().join("missing-grant");
    assert!(scan(&game, &AtomicBool::new(false)).is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn records_all_three_baselines_with_game_configuration() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let configs = (1..=3)
        .map(|number| game(temp.path(), number).map(config))
        .collect::<Result<Vec<_>>>()?;
    configure(&tracker, &configs, &[], Arc::new(|_| {})).await?;
    assert_eq!(tracker.load_persisted_games().await?.len(), 3);
    for config in &configs {
        let baseline = tracker
            .load_mele_baseline(&config.game.id)
            .await?
            .context("missing baseline")?;
        assert_eq!(baseline.files.len(), 2);
        let vanilla = tracker.get_vanilla_metadata(&config.game.id).await?;
        assert!(vanilla.contains_key("cookedpcconsole/engine.pcc"));
        assert!(vanilla.keys().any(|path| path.starts_with("../binaries/")));
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preserves_the_original_baseline_across_reopening_and_setup_changes() -> Result<()> {
    let temp = tempdir()?;
    let db_url = format!(
        "sqlite://{}?mode=rwc",
        temp.path().join("tracker.db").display()
    );
    let tracker = Tracker::open(&db_url).await?.tracker;
    let config = config(game(temp.path(), 1)?);
    configure(
        &tracker,
        std::slice::from_ref(&config),
        &[],
        Arc::new(|_| {}),
    )
    .await?;
    let original = tracker.load_mele_baseline(&config.game.id).await?;
    fs::write(
        config.game.path.join("BioGame/CookedPCConsole/Engine.pcc"),
        b"updated game",
    )?;
    fs::write(config.game.path.join("BioGame/external.pcc"), b"external")?;
    tracker.pool.close().await;
    let tracker = Tracker::open(&db_url).await?.tracker;
    ensure_baseline(&tracker, &config.game).await?;
    configure(
        &tracker,
        std::slice::from_ref(&config),
        &[],
        Arc::new(|_| {}),
    )
    .await?;
    assert_eq!(tracker.load_mele_baseline(&config.game.id).await?, original);
    let updated = scan(&config.game, &AtomicBool::new(false))?;
    assert!(tracker.save_mele_baseline(&updated).await.is_err());
    assert_eq!(tracker.load_mele_baseline(&config.game.id).await?, original);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rolls_back_games_and_baselines_on_database_failure() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let configs = (1..=3)
        .map(|number| game(temp.path(), number).map(config))
        .collect::<Result<Vec<_>>>()?;
    sqlx::query("CREATE TRIGGER fail_baseline BEFORE INSERT ON mele_baseline_files WHEN NEW.game_id='mass-effect-le3' BEGIN SELECT RAISE(FAIL, 'injected failure'); END")
        .execute(&tracker.pool).await?;
    assert!(
        configure(&tracker, &configs, &[], Arc::new(|_| {}))
            .await
            .is_err()
    );
    assert!(tracker.load_persisted_games().await?.is_empty());
    for config in &configs {
        assert!(tracker.load_mele_baseline(&config.game.id).await?.is_none());
        assert!(
            tracker
                .get_vanilla_metadata(&config.game.id)
                .await?
                .is_empty()
        );
    }
    assert!(tracker.get_setting("last_game_id").await?.is_none());
    let bindings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM game_locations")
        .fetch_one(&tracker.pool)
        .await?;
    assert_eq!(bindings, 0);
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn failed_folder_access_does_not_publish_any_family_member() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let configs = (1..=3)
        .map(|number| game(temp.path(), number).map(config))
        .collect::<Result<Vec<_>>>()?;
    fs::remove_dir_all(&configs[2].game.path)?;
    assert!(
        configure(&tracker, &configs, &[], Arc::new(|_| {}))
            .await
            .is_err()
    );
    assert!(tracker.load_persisted_games().await?.is_empty());
    assert!(
        tracker
            .load_mele_baseline(&configs[0].game.id)
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn existing_mele_games_acquire_a_baseline_once_without_reconfirmation() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let config = config(game(temp.path(), 1)?);
    tracker
        .persist_game_configs(std::slice::from_ref(&config), &[])
        .await?;
    ensure_baseline(&tracker, &config.game).await?;
    assert!(tracker.load_mele_baseline(&config.game.id).await?.is_some());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_damaged_persisted_baselines_instead_of_recapturing() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let config = config(game(temp.path(), 1)?);
    configure(
        &tracker,
        std::slice::from_ref(&config),
        &[],
        Arc::new(|_| {}),
    )
    .await?;
    sqlx::query("DELETE FROM mele_baseline_files WHERE relative_path LIKE 'BioGame/%'")
        .execute(&tracker.pool)
        .await?;
    assert!(tracker.load_mele_baseline(&config.game.id).await.is_err());
    assert!(ensure_baseline(&tracker, &config.game).await.is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn other_engines_keep_their_existing_setup_without_mele_scans() -> Result<()> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    for (index, engine) in [
        GameEngine::Bethesda,
        GameEngine::Aurora,
        GameEngine::Eclipse,
        GameEngine::REDEngine,
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("other-{index}");
        let game = Game {
            id: id.clone(),
            title: id.clone(),
            path: format!("/unavailable/game-{index}").into(),
            data_subdir: "Data".into(),
            engine,
            wine_prefix: None,
        };
        configure(&tracker, &[config(game)], &[], Arc::new(|_| {})).await?;
        assert!(tracker.load_mele_baseline(&id).await?.is_none());
    }
    assert_eq!(tracker.load_persisted_games().await?.len(), 4);
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn refuses_capture_during_location_repair_without_waiting_on_a_nested_lease() -> Result<()> {
    let temp = tempdir()?;
    let game = game(temp.path(), 1)?;
    let _lease = location_recovery::activity_lock().write_owned().await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        capture(vec![game], Arc::new(|_| {})),
    )
    .await?;
    assert!(result.is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn missing_baselines_cannot_be_recaptured_from_managed_mods() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let config = config(game(temp.path(), 1)?);
    tracker
        .persist_game_configs(std::slice::from_ref(&config), &[])
        .await?;
    sqlx::query(
        "INSERT INTO mods (id, game_id, name) VALUES ('managed', 'mass-effect-le1', 'Managed')",
    )
    .execute(&tracker.pool)
    .await?;
    assert!(ensure_baseline(&tracker, &config.game).await.is_err());
    assert!(
        configure(&tracker, &[config], &[], Arc::new(|_| {}))
            .await
            .is_err()
    );
    assert!(
        tracker
            .load_mele_baseline("mass-effect-le1")
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: snap
#[test]
fn rejects_a_linked_ancestor_of_the_selected_game_folder() -> Result<()> {
    let temp = tempdir()?;
    let mut game = game(temp.path(), 1)?;
    symlink(&game.path, temp.path().join("linked"))?;
    game.path = temp.path().join("linked");
    assert!(scan(&game, &AtomicBool::new(false)).is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn reports_each_new_game_and_saves_only_after_scanning() -> Result<()> {
    let temp = tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let configs = (1..=3)
        .map(|number| game(temp.path(), number).map(config))
        .collect::<Result<Vec<_>>>()?;
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = events.clone();
    let progress: Callback = Arc::new(move |event| collected.lock().unwrap().push(event));
    configure(&tracker, &configs, &[], progress.clone()).await?;
    {
        let events = events.lock().unwrap();
        assert_eq!(events.last().unwrap().phase, Phase::Saving);
        for index in 1..=3 {
            let game_events: Vec<_> = events.iter().filter(|event| event.index == index).collect();
            assert!(
                game_events
                    .iter()
                    .all(|event| event.count == 3 && event.game == format!("LE{index}"))
            );
            assert!(matches!(
                game_events.first().unwrap().phase,
                Phase::Discovering(0)
            ));
            assert!(game_events.iter().any(|event| matches!(event.phase, Phase::Hashing { bytes, total } if total > 0 && bytes == total)));
            assert!(matches!(
                game_events.last().unwrap().phase,
                Phase::Checking(_)
            ));
        }
    }
    events.lock().unwrap().clear();
    configure(&tracker, &configs, &[], progress).await?;
    assert_eq!(events.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap()[0].phase, Phase::Saving);
    Ok(())
}

// @variants: both
#[test]
fn reports_bytes_within_large_files_and_keeps_change_detection() -> Result<()> {
    let temp = tempdir()?;
    let game = game(temp.path(), 1)?;
    let target = game.path.join("BioGame/CookedPCConsole/Engine.pcc");
    fs::write(&target, vec![7; 256 * 1024])?;
    let mut amounts = Vec::new();
    scan_with_progress(&game, &AtomicBool::new(false), &mut |phase| {
        if let Phase::Hashing { bytes, total } = phase {
            amounts.push((bytes, total));
        }
    })?;
    assert!(
        amounts
            .iter()
            .any(|(bytes, total)| *bytes > 0 && bytes < total)
    );
    assert!(amounts.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    assert_eq!(amounts.last().unwrap().0, amounts.last().unwrap().1);
    let result = scan_with_progress(&game, &AtomicBool::new(false), &mut |phase| {
        if matches!(phase, Phase::Checking(0)) {
            fs::write(&target, b"changed during scan").unwrap();
        }
    });
    assert!(result.is_err());
    Ok(())
}
