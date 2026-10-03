use super::*;
use crate::models::game::GameEngine;
use crate::models::manifest::ModFile;

async fn fixture(root: &Path) -> Result<(Tracker, Game, PathBuf, PathBuf)> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let cache = root.join("cache");
    let data = root.join("data");
    let game = Game {
        id: "removal-game".into(),
        title: "Game".into(),
        path: root.join("game"),
        data_subdir: "Data".into(),
        engine: GameEngine::Bethesda,
        wine_prefix: None,
    };
    fs::create_dir_all(game.data_dir())?;
    fs::create_dir_all(cache.join("mod"))?;
    fs::create_dir_all(data.join("saves/removal-game"))?;
    fs::write(data.join("saves/removal-game/save"), b"saved game")?;
    fs::write(game.data_dir().join("mod.txt"), b"mod")?;
    fs::write(game.data_dir().join("vanilla.txt"), b"original")?;
    fs::write(cache.join("mod/mod.txt"), b"mod")?;
    let archive = root.join("archive.zip");
    fs::write(&archive, b"download")?;
    tracker
        .upsert_game(
            &game.id,
            &game.title,
            &game.path,
            &game.data_subdir,
            game.engine.as_str(),
            None,
            true,
        )
        .await?;
    tracker.ensure_default_profile(&game.id).await?;
    tracker
        .set_setting(&format!("cache_dir_{}", game.id), cache.to_str().unwrap())
        .await?;
    sqlx::query("INSERT INTO mods(id,game_id,name,archive_hash) VALUES ('mod',?,'Mod','download')")
        .bind(&game.id)
        .execute(&tracker.pool)
        .await?;
    sqlx::query("INSERT INTO download_entries(id,mod_name,archive_hash,archive_path,status) VALUES ('archive','Mod','download',?,'installed')").bind(archive.to_str()).execute(&tracker.pool).await?;
    tracker
        .record_deployed_files(
            &game.id,
            &[ModFile {
                mod_id: "mod".into(),
                game_rel_lowercase: "mod.txt".into(),
                game_rel_original: "mod.txt".into(),
                cache_path: cache.join("mod/mod.txt").to_string_lossy().into_owned(),
            }],
        )
        .await?;
    Ok((tracker, game, cache, data))
}

// @variants: both
#[tokio::test]
async fn full_reset_purges_and_readds_without_old_state_while_preserving_saves_and_archives()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, cache, data) = fixture(temp.path()).await?;
    fs::create_dir(cache.join("other"))?;
    fs::write(cache.join("other/keep"), b"other mod")?;
    sqlx::query("INSERT INTO mods(id,game_id,name) VALUES ('other','other-game','Other')")
        .execute(&tracker.pool)
        .await?;
    remove_in(&tracker, &game, &cache, data.clone(), Control::default()).await?;
    assert!(!game.data_dir().join("mod.txt").exists());
    assert_eq!(fs::read(game.data_dir().join("vanilla.txt"))?, b"original");
    assert_eq!(
        fs::read(data.join("saves/removal-game/save"))?,
        b"saved game"
    );
    assert_eq!(fs::read(temp.path().join("archive.zip"))?, b"download");
    assert_eq!(fs::read(cache.join("other/keep"))?, b"other mod");
    assert!(!cache.join("mod").exists());
    assert!(!cache.join("deployd-history").join(&game.id).exists());
    for table in [
        "profiles",
        "generations",
        "generation_objects",
        "generation_stores",
        "deployed_files",
        "game_locations",
        "vanilla_backups",
    ] {
        let count: i64 =
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE game_id=?"))
                .bind(&game.id)
                .fetch_one(&tracker.pool)
                .await?;
        assert_eq!(count, 0, "{table}");
    }
    assert!(tracker.load_persisted_games().await?.is_empty());
    assert_eq!(tracker.load_hidden_game_ids().await?, [game.id.clone()]);
    assert!(
        tracker
            .get_setting(&format!("cache_dir_{}", game.id))
            .await?
            .is_none()
    );
    let status: String =
        sqlx::query_scalar("SELECT status FROM download_entries WHERE id='archive'")
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(status, "downloaded");
    tracker
        .upsert_game(
            &game.id,
            &game.title,
            &game.path,
            &game.data_subdir,
            game.engine.as_str(),
            None,
            true,
        )
        .await?;
    assert!(tracker.list_mods(&game.id).await?.is_empty());
    assert!(tracker.list_profiles(&game.id).await?.is_empty());
    assert!(tracker.get_deployed_files(&game.id).await?.is_empty());
    assert_eq!(tracker.list_mods("other-game").await?.len(), 1);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_purge_preserves_mod_cache_and_database_for_retry() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, cache, data) = fixture(temp.path()).await?;
    sqlx::query("CREATE TRIGGER fail_purge BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected purge failure'); END").execute(&tracker.pool).await?;
    assert!(
        remove_in(&tracker, &game, &cache, data, Control::default())
            .await
            .is_err()
    );
    assert_eq!(tracker.list_mods(&game.id).await?.len(), 1);
    assert_eq!(tracker.list_profiles(&game.id).await?.len(), 1);
    assert_eq!(fs::read(cache.join("mod/mod.txt"))?, b"mod");
    assert_eq!(tracker.get_deployed_files(&game.id).await?.len(), 1);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn interrupted_cleanup_retries_after_restart_without_purging_again() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, cache, data) = fixture(temp.path()).await?;
    sqlx::query("CREATE TRIGGER fail_removal BEFORE UPDATE OF path ON games WHEN NEW.path IS NULL BEGIN SELECT RAISE(ABORT,'injected commit failure'); END").execute(&tracker.pool).await?;
    assert!(
        remove_in(&tracker, &game, &cache, data.clone(), Control::default())
            .await
            .is_err()
    );
    assert!(
        tracker
            .get_setting(&format!("{PREFIX}{}", game.id))
            .await?
            .is_some()
    );
    assert!(tracker.ensure_location_ready(&game.id).await.is_err());
    assert!(tracker.remove_managed_game(&game.id, true).await.is_err());
    assert!(
        tracker
            .upsert_game(
                &game.id,
                &game.title,
                &game.path,
                &game.data_subdir,
                game.engine.as_str(),
                None,
                true
            )
            .await
            .is_err()
    );
    assert!(!cache.join("mod").exists());
    assert_eq!(tracker.list_mods(&game.id).await?.len(), 1);
    sqlx::query("DROP TRIGGER fail_removal")
        .execute(&tracker.pool)
        .await?;
    let offline = temp.path().join("offline-cache");
    fs::rename(&cache, &offline)?;
    assert_eq!(resume_all(&tracker).await?.len(), 1);
    assert!(
        tracker
            .get_setting(&format!("{PREFIX}{}", game.id))
            .await?
            .is_some()
    );
    assert_eq!(tracker.list_mods(&game.id).await?.len(), 1);
    fs::rename(&offline, &cache)?;
    assert!(resume_all(&tracker).await?.is_empty());
    assert!(
        tracker
            .get_setting(&format!("{PREFIX}{}", game.id))
            .await?
            .is_none()
    );
    assert!(tracker.list_mods(&game.id).await?.is_empty());
    assert_eq!(fs::read(temp.path().join("archive.zip"))?, b"download");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn cleanup_refuses_archives_inside_managed_storage() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, cache, data) = fixture(temp.path()).await?;
    let archive = cache.join("mod/archive.zip");
    fs::write(&archive, b"keep archive")?;
    sqlx::query("UPDATE download_entries SET archive_path=?")
        .bind(archive.to_str())
        .execute(&tracker.pool)
        .await?;
    let error = remove_in(&tracker, &game, &cache, data, Control::default())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("downloaded archive"));
    assert_eq!(fs::read(archive)?, b"keep archive");
    assert_eq!(tracker.list_mods(&game.id).await?.len(), 1);
    Ok(())
}

// @variants: both
#[test]
fn cleanup_refuses_symlinked_storage_and_keeps_other_named_links() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    fs::create_dir_all(cache.join("named_mods/by-name"))?;
    fs::create_dir(temp.path().join("outside"))?;
    fs::write(temp.path().join("outside/keep"), b"keep")?;
    std::os::unix::fs::symlink(temp.path().join("outside"), cache.join("mod"))?;
    assert!(check_ancestors(&cache.join("mod")).is_err());
    std::os::unix::fs::symlink(cache.join("mod"), cache.join("named_mods/remove"))?;
    std::os::unix::fs::symlink(cache.join("other"), cache.join("named_mods/keep"))?;
    remove_named_links(&cache, &["mod".into()])?;
    assert!(fs::symlink_metadata(cache.join("named_mods/remove")).is_err());
    assert!(fs::symlink_metadata(cache.join("named_mods/keep")).is_ok());
    assert_eq!(fs::read(temp.path().join("outside/keep"))?, b"keep");
    Ok(())
}

async fn trilogy_fixture(
    root: &Path,
) -> Result<(
    Tracker,
    Vec<crate::models::game::GameConfig>,
    PathBuf,
    PathBuf,
)> {
    use crate::core::game::mass_effect::baseline;
    use crate::models::game::GameConfig;
    use crate::utils::location::{FolderRole, FolderSelection, SelectedLocation};

    let tracker = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        root.join("tracker.db").display()
    ))
    .await?
    .tracker;
    let family = root.join("family");
    fs::create_dir_all(family.join("Game/Launcher"))?;
    fs::write(
        family.join("Game/Launcher/MassEffectLauncher.exe"),
        b"launcher",
    )?;
    fs::write(
        family.join("Game/Launcher/bink2w64.dll"),
        b"original launcher",
    )?;
    let mut configs = Vec::new();
    for number in 1..=3 {
        let game = Game {
            id: format!("mass-effect-le{number}"),
            title: format!("LE{number}"),
            path: family.join(format!("Game/ME{number}")),
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        fs::create_dir_all(game.path.join("Binaries/Win64"))?;
        fs::create_dir_all(game.data_dir())?;
        fs::write(
            game.path
                .join(format!("Binaries/Win64/MassEffect{number}.exe")),
            b"exe",
        )?;
        fs::write(game.path.join("Binaries/Win64/bink2w64.dll"), b"bink")?;
        configs.push(GameConfig {
            game,
            custom: true,
            locations: vec![FolderSelection {
                role: FolderRole::Game,
                location: SelectedLocation {
                    root: family.clone(),
                    host_hint: None,
                },
                relative: format!("Game/ME{number}").into(),
            }],
        });
    }
    baseline::configure(&tracker, &configs, &[], std::sync::Arc::new(|_| {})).await?;
    let cache = root.join("cache");
    let data = root.join("data");
    fs::create_dir(&cache)?;
    fs::create_dir(&data)?;
    for config in &configs {
        tracker.ensure_default_profile(&config.game.id).await?;
        tracker
            .set_setting(
                &format!("cache_dir_{}", config.game.id),
                cache.to_str().unwrap(),
            )
            .await?;
        let id = &config.game.id;
        sqlx::query("INSERT INTO mods(id,game_id,name) VALUES (?,?,'Mod')")
            .bind(id)
            .bind(id)
            .execute(&tracker.pool)
            .await?;
        fs::create_dir(cache.join(id))?;
        fs::write(cache.join(id).join("keep-until-purged"), b"cached")?;
        fs::create_dir_all(data.join("saves").join(id))?;
        fs::write(data.join("saves").join(id).join("save"), b"saved")?;
    }
    Ok((tracker, configs, cache, data))
}

// @variants: both
#[tokio::test]
async fn removing_any_mele_game_removes_the_trilogy_and_readds_from_scratch() -> Result<()> {
    for selected in 0..3 {
        let temp = tempfile::tempdir()?;
        let (tracker, configs, cache, data) = trilogy_fixture(temp.path()).await?;
        remove_in(
            &tracker,
            &configs[selected].game,
            &cache,
            data.clone(),
            Control::default(),
        )
        .await?;
        for config in &configs {
            let id = &config.game.id;
            assert!(tracker.load_mele_baseline(id).await?.is_none());
            assert!(tracker.get_active_profile(id).await?.is_none());
            assert!(tracker.list_mods(id).await?.is_empty());
            assert!(!cache.join(id).exists());
            assert_eq!(
                fs::read(data.join("saves").join(id).join("save"))?,
                b"saved"
            );
        }
        assert!(tracker.load_persisted_games().await?.is_empty());
        let families: i64 = sqlx::query_scalar("SELECT count(*) FROM mele_families")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(families, 0);
        assert_eq!(
            fs::read(temp.path().join("family/Game/Launcher/bink2w64.dll"))?,
            b"original launcher"
        );
        crate::core::game::mass_effect::baseline::configure(
            &tracker,
            &configs,
            &[],
            std::sync::Arc::new(|_| {}),
        )
        .await?;
        assert_eq!(tracker.load_persisted_games().await?.len(), 3);
    }
    Ok(())
}

async fn record_trilogy_deployments(
    tracker: &Tracker,
    configs: &[crate::models::game::GameConfig],
) -> Result<()> {
    for (index, config) in configs.iter().enumerate() {
        let profile = tracker
            .get_active_profile(&config.game.id)
            .await?
            .context("Missing profile")?;
        let state = serde_json::json!({
            "version": 1, "generation": uuid::Uuid::new_v4().to_string(), "profile": profile.id,
            "files": [], "recipe": {"version":1,"target":format!("LE{}",index+1),"backend_version":1,"helper_version":null,"language":"INT","packages":[],"components":[]}
        });
        sqlx::query("INSERT INTO mele_deployments(game_id,document) VALUES (?,?)")
            .bind(&config.game.id)
            .bind(state.to_string())
            .execute(&tracker.pool)
            .await?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_trilogy_purge_keeps_every_game_managed_and_retry_skips_finished_purges()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, configs, cache, data) = trilogy_fixture(temp.path()).await?;
    record_trilogy_deployments(&tracker, &configs).await?;
    sqlx::query("CREATE TRIGGER reject_second_purge BEFORE INSERT ON generation_activations WHEN NEW.game_id='mass-effect-le2' BEGIN SELECT RAISE(ABORT,'injected second purge failure'); END").execute(&tracker.pool).await?;
    let result = remove_in(
        &tracker,
        &configs[2].game,
        &cache,
        data.clone(),
        Control::default(),
    )
    .await;
    assert!(result.is_err());
    assert!(format!("{:#}", result.unwrap_err()).contains("injected second purge failure"));
    assert_eq!(tracker.load_persisted_games().await?.len(), 3);
    for config in &configs {
        assert!(tracker.load_mele_baseline(&config.game.id).await?.is_some());
        assert!(tracker.get_active_profile(&config.game.id).await?.is_some());
        assert_eq!(tracker.list_mods(&config.game.id).await?.len(), 1);
        assert!(
            cache
                .join(&config.game.id)
                .join("keep-until-purged")
                .exists()
        );
        assert!(
            tracker
                .get_setting(&format!("{PREFIX}{}", config.game.id))
                .await?
                .is_none()
        );
    }
    assert!(
        tracker
            .mele_deployment(&configs[0].game.id)
            .await?
            .unwrap()
            .recipe
            .is_none()
    );
    assert!(
        tracker
            .mele_deployment(&configs[1].game.id)
            .await?
            .unwrap()
            .recipe
            .is_some()
    );
    sqlx::query("DROP TRIGGER reject_second_purge")
        .execute(&tracker.pool)
        .await?;
    sqlx::query("CREATE TRIGGER reject_repeated_first_purge BEFORE INSERT ON generation_activations WHEN NEW.game_id='mass-effect-le1' BEGIN SELECT RAISE(ABORT,'first purge repeated'); END").execute(&tracker.pool).await?;
    remove_in(&tracker, &configs[1].game, &cache, data, Control::default()).await?;
    assert!(tracker.load_persisted_games().await?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn interrupted_trilogy_cleanup_retains_all_records_and_resumes_as_one_operation() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let (tracker, configs, cache, data) = trilogy_fixture(temp.path()).await?;
    sqlx::query("CREATE TRIGGER reject_last_removal BEFORE UPDATE OF path ON games WHEN NEW.id='mass-effect-le3' AND NEW.path IS NULL BEGIN SELECT RAISE(ABORT,'injected final failure'); END").execute(&tracker.pool).await?;
    assert!(
        remove_in(&tracker, &configs[0].game, &cache, data, Control::default())
            .await
            .is_err()
    );
    assert_eq!(tracker.load_persisted_games().await?.len(), 3);
    for config in &configs {
        assert!(tracker.get_active_profile(&config.game.id).await?.is_some());
        assert!(
            tracker
                .get_setting(&format!("{PREFIX}{}", config.game.id))
                .await?
                .is_some()
        );
        assert!(
            tracker
                .ensure_location_ready(&config.game.id)
                .await
                .is_err()
        );
    }
    sqlx::query("DROP TRIGGER reject_last_removal")
        .execute(&tracker.pool)
        .await?;
    drop(tracker);
    let tracker = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    assert!(resume_all(&tracker).await?.is_empty());
    assert!(tracker.load_persisted_games().await?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn full_reset_restores_backed_up_originals_before_deleting_backups() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, cache, data) = fixture(temp.path()).await?;
    let backup = data.join(&game.id).join("vanilla-backup/mod.txt");
    fs::create_dir_all(backup.parent().unwrap())?;
    fs::write(&backup, b"original file")?;
    tracker
        .save_vanilla_backup(&game.id, "mod.txt", "mod.txt", &backup)
        .await?;
    remove_in(&tracker, &game, &cache, data, Control::default()).await?;
    assert_eq!(fs::read(game.data_dir().join("mod.txt"))?, b"original file");
    assert!(!backup.exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn full_reset_keeps_records_when_an_original_cannot_be_restored() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, cache, data) = fixture(temp.path()).await?;
    sqlx::query("INSERT INTO vanilla_files(game_id,game_rel_lowercase,file_size,mtime_secs) VALUES (?,'mod.txt',3,0)").bind(&game.id).execute(&tracker.pool).await?;
    let error = remove_in(&tracker, &game, &cache, data, Control::default())
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("No original backup"));
    assert_eq!(fs::read(game.data_dir().join("mod.txt"))?, b"mod");
    assert_eq!(tracker.list_mods(&game.id).await?.len(), 1);
    assert!(cache.join("mod/mod.txt").exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn legacy_cache_deletion_preserves_archives_inside_mod_directories() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, _, cache, _) = fixture(temp.path()).await?;
    let archive = cache.join("mod/download.zip");
    fs::write(&archive, b"archive")?;
    sqlx::query("UPDATE download_entries SET archive_path=? WHERE id='archive'")
        .bind(archive.to_str())
        .execute(&tracker.pool)
        .await?;
    assert!(delete_mod_cache(&tracker, &cache, "mod").await.is_err());
    assert_eq!(fs::read(archive)?, b"archive");
    assert!(cache.join("mod/mod.txt").exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn trilogy_removal_uses_each_games_own_cache_directory() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, configs, cache, data) = trilogy_fixture(temp.path()).await?;
    let mut roots = Vec::new();
    for (index, config) in configs.iter().enumerate() {
        let root = temp.path().join(format!("cache-{index}"));
        fs::create_dir(&root)?;
        fs::rename(cache.join(&config.game.id), root.join(&config.game.id))?;
        tracker
            .set_setting(
                &format!("cache_dir_{}", config.game.id),
                root.to_str().unwrap(),
            )
            .await?;
        roots.push(root);
    }
    fs::write(cache.join("unrelated"), b"keep")?;
    remove_in(
        &tracker,
        &configs[1].game,
        &roots[1],
        data,
        Control::default(),
    )
    .await?;
    for (config, root) in configs.iter().zip(roots) {
        assert!(!root.join(&config.game.id).exists());
        assert!(!root.join("deployd-history").join(&config.game.id).exists());
    }
    assert_eq!(fs::read(cache.join("unrelated"))?, b"keep");
    Ok(())
}
