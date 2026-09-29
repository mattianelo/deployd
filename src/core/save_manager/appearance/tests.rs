use std::{future::Future, process::Command};

use super::*;
use crate::models::game::{GameConfig, GameEngine};

fn isolated<F: Future<Output = Result<()>>>(
    name: &str,
    run: impl FnOnce(PathBuf) -> F,
) -> Result<()> {
    if std::env::var("DEPLOYD_APPEARANCE_CASE").as_deref() == Ok(name) {
        let root = PathBuf::from(std::env::var("DEPLOYD_APPEARANCE_ROOT")?);
        ensure!(
            crate::utils::paths::deployd_data_dir()?.starts_with(&root),
            "Test storage escaped isolation"
        );
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(run(root));
    }
    for snap in [false, true] {
        let temp = tempfile::tempdir()?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                &format!("core::save_manager::appearance::tests::{name}"),
                "--nocapture",
            ])
            .env("DEPLOYD_APPEARANCE_CASE", name)
            .env("DEPLOYD_APPEARANCE_ROOT", temp.path())
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env_remove("SNAP")
            .env_remove("SNAP_NAME")
            .env_remove("SNAP_USER_DATA")
            .env_remove("SNAP_USER_COMMON");
        if snap {
            command.env("SNAP_USER_COMMON", temp.path().join("common"));
        }
        let output = command.output()?;
        ensure!(
            output.status.success(),
            "Appearance case failed (Snap storage {snap}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

async fn fixture(root: &Path) -> Result<(Tracker, Game, PathBuf)> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let game = Game {
        id: Target::Le2.game_id().into(),
        title: Target::Le2.label().into(),
        path: root.join("game"),
        data_subdir: "BioGame".into(),
        engine: GameEngine::MassEffect,
        wine_prefix: Some(root.join("prefix")),
    };
    let live = root.join(
        "prefix/drive_c/users/steamuser/Documents/BioWare/Mass Effect Legendary Edition/Save/ME2",
    );
    fs::create_dir_all(&live)?;
    fs::create_dir_all(&game.path)?;
    fs::write(
        live.join("Save_0001.pcsav"),
        include_bytes!("../../../../tests/fixtures/appearance/ME2LeSave.pcsav"),
    )?;
    tracker
        .persist_game_configs(
            &[GameConfig {
                game: game.clone(),
                custom: true,
                locations: Vec::new(),
            }],
            &[],
        )
        .await?;
    tracker.ensure_default_profile(&game.id).await?;
    generations::session::initialize(&tracker, &game).await?;
    Ok((tracker, game, live))
}

// @variants: both
#[test]
fn saves_with_a_durable_backup_and_captures_the_edited_live_state() -> Result<()> {
    isolated(
        "saves_with_a_durable_backup_and_captures_the_edited_live_state",
        |root| async move {
            let (tracker, game, live) = fixture(&root).await?;
            let listing = list(&tracker, &game).await?;
            assert_eq!(listing.entries.len(), 1);
            let mut session =
                open(&tracker, game.clone(), listing.entries[0].relative.clone()).await?;
            let original = session.document.original().to_vec();
            let morph = session
                .document
                .morph
                .as_mut()
                .context("Fixture has no morph")?;
            morph.hair_mesh = "TestPackage.Hair.Mesh".into();
            let saved = save(tracker.clone(), session).await?;
            assert!(!saved.document.changed());
            assert_ne!(fs::read(live.join("Save_0001.pcsav"))?, original);
            let backups = super::super::list_backups(&game.id).await?;
            assert_eq!(backups.len(), 1);
            let backup = super::super::backup_root(&game.id, &backups[0].backup_id)?;
            assert_eq!(fs::read(backup.join("data/Save_0001.pcsav"))?, original);
            assert!(!journal_path(&game)?.exists());
            super::super::capture_save_set(
                &game,
                &saved.owner,
                super::super::BackupTrigger::ProfileSwitch,
                u64::MAX,
            )
            .await?;
            let bank = super::super::bank_root(&saved.owner)?;
            assert_eq!(
                fs::read(bank.join("data/Save_0001.pcsav"))?,
                saved.document.original()
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn refuses_external_edits_and_changed_save_owners() -> Result<()> {
    isolated(
        "refuses_external_edits_and_changed_save_owners",
        |root| async move {
            let (tracker, game, live) = fixture(&root).await?;
            let session = open(&tracker, game.clone(), "Save_0001.pcsav".into()).await?;
            fs::write(live.join("Save_0001.pcsav"), b"external progress")?;
            assert!(save(tracker.clone(), session.clone()).await.is_err());
            assert_eq!(
                fs::read(live.join("Save_0001.pcsav"))?,
                b"external progress"
            );
            fs::write(live.join("Save_0001.pcsav"), session.document.original())?;
            let session = open(&tracker, game.clone(), "Save_0001.pcsav".into()).await?;
            let profile = tracker.ensure_default_profile(&game.id).await?;
            sqlx::query("UPDATE generation_game_state SET live_save_mode='profile',live_save_profile_id=? WHERE game_id=?").bind(&profile.id).bind(&game.id).execute(&tracker.pool).await?;
            assert!(save(tracker.clone(), session).await.is_err());
            assert!(super::super::list_backups(&game.id).await?.is_empty());
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn refuses_lost_access_and_changed_prefix_bindings() -> Result<()> {
    isolated(
        "refuses_lost_access_and_changed_prefix_bindings",
        |root| async move {
            let (tracker, game, _) = fixture(&root).await?;
            let session = open(&tracker, game.clone(), "Save_0001.pcsav".into()).await?;
            fs::rename(root.join("prefix"), root.join("lost-prefix"))?;
            assert!(save(tracker.clone(), session.clone()).await.is_err());
            fs::rename(root.join("lost-prefix"), root.join("prefix"))?;
            let location = tracker
                .folder_location(&game.id, FolderRole::Prefix)
                .await?;
            sqlx::query("UPDATE folder_locations SET root=? WHERE id=?")
                .bind(root.join("other").to_string_lossy().as_ref())
                .bind(location.id)
                .execute(&tracker.pool)
                .await?;
            assert!(save(tracker, session).await.is_err());
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn recovers_staged_and_committed_edits_without_overwriting_external_changes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let live = root.join("Save.pcsav");
    let staged = root.join(".deployd-appearance-test.tmp");
    let record = root.join("journal.json");
    let journal = Journal {
        version: 1,
        owner: SaveSetId::Global {
            game_id: "game".into(),
        },
        relative: "Save.pcsav".into(),
        staging: ".deployd-appearance-test.tmp".into(),
        before: hash(b"before"),
        after: hash(b"after"),
    };
    fs::write(&live, b"before")?;
    fs::write(&staged, b"after")?;
    fs::write(&record, serde_json::to_vec(&journal)?)?;
    finish(root, &record, &journal)?;
    assert_eq!(fs::read(&live)?, b"before");
    assert!(!staged.exists());
    assert!(!record.exists());
    fs::write(&staged, b"partial write")?;
    fs::write(&record, serde_json::to_vec(&journal)?)?;
    finish(root, &record, &journal)?;
    assert_eq!(fs::read(&live)?, b"before");
    assert!(!staged.exists());
    fs::write(&live, b"after")?;
    fs::write(&record, serde_json::to_vec(&journal)?)?;
    finish(root, &record, &journal)?;
    assert_eq!(fs::read(&live)?, b"after");
    fs::write(&live, b"external")?;
    fs::write(&record, serde_json::to_vec(&journal)?)?;
    assert!(finish(root, &record, &journal).is_err());
    assert!(record.exists());
    assert_eq!(fs::read(&live)?, b"external");
    Ok(())
}

#[test]
fn rejects_traversal_symlinks_and_hardlinked_saves() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(root.join("save"), b"data")?;
    assert!(resolve(root, Path::new("../save")).is_err());
    std::os::unix::fs::symlink(root.join("save"), root.join("linked"))?;
    assert!(resolve(root, Path::new("linked")).is_err());
    assert!(read(&root.join("linked")).is_err());
    fs::hard_link(root.join("save"), root.join("hard"))?;
    assert!(read(&root.join("save")).is_err());
    Ok(())
}

// @variants: both
#[test]
fn refuses_editing_during_pending_recovery() -> Result<()> {
    isolated(
        "refuses_editing_during_pending_recovery",
        |root| async move {
            let (tracker, game, _) = fixture(&root).await?;
            let session = open(&tracker, game.clone(), "Save_0001.pcsav".into()).await?;
            sqlx::query(
                "INSERT INTO mele_journals(game_id,id,document) VALUES(?, 'pending', '{}')",
            )
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
            assert!(list(&tracker, &game).await.is_err());
            assert!(
                open(&tracker, game.clone(), "Save_0001.pcsav".into())
                    .await
                    .is_err()
            );
            assert!(save(tracker.clone(), session.clone()).await.is_err());
            sqlx::query("DELETE FROM mele_journals")
                .execute(&tracker.pool)
                .await?;
            let game_root = super::super::game_root(&game.id)?;
            fs::create_dir_all(&game_root)?;
            let record = game_root.join("transition.json");
            fs::write(&record, b"pending")?;
            assert!(list(&tracker, &game).await.is_err());
            assert!(save(tracker.clone(), session).await.is_err());
            assert!(super::super::list_backups(&game.id).await?.is_empty());
            fs::remove_file(record)?;
            assert!(list(&tracker, &game).await.is_ok());
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn excludes_other_location_operations_while_editing() -> Result<()> {
    isolated(
        "excludes_other_location_operations_while_editing",
        |root| async move {
            let (tracker, game, _) = fixture(&root).await?;
            let _other = crate::core::location_recovery::activity_lock()
                .read_owned()
                .await;
            assert!(
                open(&tracker, game.clone(), "Save_0001.pcsav".into())
                    .await
                    .is_err()
            );
            assert!(list(&tracker, &game).await.is_err());
            Ok(())
        },
    )
}
