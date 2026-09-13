use std::fs;

use anyhow::Result;

use crate::core::save_manager::SaveSetId;
use crate::models::manifest::ModFile;

use super::catalog::{History, durable};
use super::content::Control;
use super::journal::{Journal, Node};
use super::manifest::{self, Manifest};
use super::state::{self, Deployment};

async fn unpublished(
    history: &History,
    game: &crate::models::game::Game,
    profile: &str,
) -> Result<(Manifest, Vec<ModFile>, Journal)> {
    let mut manifest = manifest::capture(
        history,
        game,
        profile,
        history.cache.clone(),
        Control::default(),
    )
    .await?;
    manifest.outputs = super::prepared::files(&manifest)?;
    let mut desired = Vec::new();
    let mut files = Vec::new();
    for output in &manifest.outputs {
        let node = Node::File {
            identity: output.content.clone().expect("fixture file"),
            mode: output.mode,
        };
        desired.push((output.target.clone(), node));
        files.push(ModFile {
            mod_id: "winner".into(),
            game_rel_lowercase: "file.txt".into(),
            game_rel_original: "File.txt".into(),
            cache_path: history
                .cache
                .join("winner/file.txt")
                .to_string_lossy()
                .into_owned(),
        });
    }
    let journal = Journal::prepare(history, game, desired, Control::default()).await?;
    Ok((manifest, files, journal))
}

async fn prepared(
    history: &History,
    game: &crate::models::game::Game,
    profile: &str,
) -> Result<(Manifest, Vec<ModFile>, Journal)> {
    let (manifest, files, journal) = unpublished(history, game, profile).await?;
    journal
        .persist(history, game, "deploy", manifest.objects())
        .await?;
    Ok((manifest, files, journal))
}

// @variants: both
#[tokio::test]
async fn failed_activation_commit_preserves_previous_state_and_recovers_files() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    fs::write(game.data_dir().join("File.txt"), b"before")?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (manifest, files, journal) = prepared(&history, &game, &profile).await?;
    let applied = journal.apply(&history, &game, Control::default()).await?;
    sqlx::query("CREATE TRIGGER reject_activation BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected commit failure'); END").execute(&tracker.pool).await?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    assert!(
        applied
            .commit(
                &history,
                &game,
                None,
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files
                }),
                &saves
            )
            .await
            .is_err()
    );
    let mut tx = durable(&tracker).await?;
    assert!(state::read(&mut tx, &game.id).await?.is_none());
    tx.rollback().await?;
    assert!(tracker.get_deployed_files(&game.id).await?.is_empty());
    assert!(
        tracker
            .get_setting(&format!("last_deployed_profile_{}", game.id))
            .await?
            .is_none()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    let committed: bool =
        sqlx::query_scalar("SELECT committed FROM generation_journals WHERE id=?")
            .bind(&journal.id)
            .fetch_one(&tracker.pool)
            .await?;
    assert!(!committed);
    journal.recover(&history, &game, false).await?;
    assert_eq!(fs::read(game.data_dir().join("File.txt"))?, b"before");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn unchanged_generation_records_separate_activations_and_purge_retains_history_and_saves()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
        .bind(&profile)
        .execute(&tracker.pool)
        .await?;
    let saves = SaveSetId::Profile {
        game_id: game.id.clone(),
        profile_id: profile.clone(),
    };
    let mut previous = None;
    let mut generation = None;
    for _ in 0..2 {
        let (manifest, files, journal) = prepared(&history, &game, &profile).await?;
        let applied = journal.apply(&history, &game, Control::default()).await?;
        applied
            .commit(
                &history,
                &game,
                previous.as_ref(),
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files,
                }),
                &saves,
            )
            .await?;
        journal.recover(&history, &game, true).await?;
        let mut tx = durable(&tracker).await?;
        previous = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        let id = manifest.id()?;
        if let Some(generation) = &generation {
            assert_eq!(generation, &id);
        }
        generation = Some(id);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_activations")
            .fetch_one(&tracker.pool)
            .await?,
        2
    );
    assert!(
        sqlx::query("DELETE FROM profiles WHERE id=?")
            .bind(&profile)
            .execute(&tracker.pool)
            .await
            .is_err()
    );
    let target = super::target::Target::file(&game.engine, "File.txt")?;
    let purge = Journal::prepare(
        &history,
        &game,
        vec![(target, Node::Absent)],
        Control::default(),
    )
    .await?;
    purge
        .persist(&history, &game, "purge", Default::default())
        .await?;
    purge
        .apply(&history, &game, Control::default())
        .await?
        .commit(&history, &game, previous.as_ref(), None, &saves)
        .await?;
    purge.recover(&history, &game, true).await?;
    let mut tx = durable(&tracker).await?;
    let state = state::read(&mut tx, &game.id).await?.expect("purged state");
    tx.rollback().await?;
    assert!(state.generation.is_none() && state.profile.is_none());
    assert_eq!(state.saves, saves);
    assert!(!game.data_dir().join("File.txt").exists());
    assert!(tracker.get_deployed_files(&game.id).await?.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        1
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn external_changes_after_application_block_the_commit_decision() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (manifest, files, journal) = prepared(&history, &game, &profile).await?;
    let applied = journal.apply(&history, &game, Control::default()).await?;
    fs::write(game.data_dir().join("File.txt"), b"external edit")?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    assert!(
        applied
            .commit(
                &history,
                &game,
                None,
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files
                }),
                &saves
            )
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_activations")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    assert!(journal.recover(&history, &game, false).await.is_err());
    assert_eq!(
        fs::read(game.data_dir().join("File.txt"))?,
        b"external edit"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn incomplete_deployment_records_cannot_commit_a_generation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (manifest, _, journal) = prepared(&history, &game, &profile).await?;
    let applied = journal.apply(&history, &game, Control::default()).await?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    assert!(
        applied
            .commit(
                &history,
                &game,
                None,
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &[]
                }),
                &saves
            )
            .await
            .is_err()
    );
    journal.recover(&history, &game, false).await?;
    assert!(!game.data_dir().join("File.txt").exists());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn changed_required_base_inputs_block_historical_commit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let base = game.data_dir().join("Base.bin");
    fs::write(&base, b"required vanilla bytes")?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (mut manifest, files, journal) = prepared(&history, &game, &profile).await?;
    manifest.base_inputs.push(super::manifest::Output {
        target: super::target::Target::file(&game.engine, "Base.bin")?,
        content: Some(super::content::inspect(&base, &Control::default())?),
        mode: 0o644,
        mod_id: None,
    });
    let applied = journal.apply(&history, &game, Control::default()).await?;
    fs::write(&base, b"game updated")?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    assert!(
        applied
            .commit(
                &history,
                &game,
                None,
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files
                }),
                &saves
            )
            .await
            .is_err()
    );
    journal.recover(&history, &game, false).await?;
    assert_eq!(fs::read(base)?, b"game updated");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn deployment_records_cannot_assign_a_winner_to_another_mod() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (manifest, mut files, journal) = prepared(&history, &game, &profile).await?;
    files[0].mod_id = "loser".into();
    let applied = journal.apply(&history, &game, Control::default()).await?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    assert!(
        applied
            .commit(
                &history,
                &game,
                None,
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files
                }),
                &saves
            )
            .await
            .is_err()
    );
    journal.recover(&history, &game, false).await?;
    assert!(tracker.get_deployed_files(&game.id).await?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn changed_save_ownership_blocks_a_stale_activation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (manifest, files, journal) = prepared(&history, &game, &profile).await?;
    let applied = journal.apply(&history, &game, Control::default()).await?;
    sqlx::query("INSERT INTO generation_game_state(game_id,live_save_profile_id,live_save_mode) VALUES (?,?,'profile')").bind(&game.id).bind(&profile).execute(&tracker.pool).await?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    assert!(
        applied
            .commit(
                &history,
                &game,
                None,
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files
                }),
                &saves
            )
            .await
            .is_err()
    );
    journal.recover(&history, &game, false).await?;
    let mut tx = durable(&tracker).await?;
    let state = state::read(&mut tx, &game.id)
        .await?
        .expect("existing save owner");
    tx.rollback().await?;
    assert_eq!(state.saves.profile_id(), Some(profile.as_str()));
    assert!(state.generation.is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn consumed_base_inputs_are_validated_against_the_retained_before_state() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let base = game.data_dir().join("File.txt");
    fs::write(&base, b"required original")?;
    let expected = super::content::inspect(&base, &Control::default())?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let (mut manifest, files, journal) = prepared(&history, &game, &profile).await?;
    manifest.base_inputs.push(super::manifest::Output {
        target: super::target::Target::file(&game.engine, "File.txt")?,
        content: Some(expected),
        mode: 0o644,
        mod_id: None,
    });
    let applied = journal.apply(&history, &game, Control::default()).await?;
    let saves = SaveSetId::Global {
        game_id: game.id.clone(),
    };
    applied
        .commit(
            &history,
            &game,
            None,
            Some(&Deployment {
                manifest: &manifest,
                profile: &profile,
                files: &files,
            }),
            &saves,
        )
        .await?;
    journal.recover(&history, &game, true).await?;
    assert_eq!(fs::read(base)?, b"winner");
    assert_eq!(history.load(&manifest.id()?).await?, manifest);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn saves_and_deployed_files_recover_from_the_same_commit_decision() -> Result<()> {
    for committed in [false, true] {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) = super::tests::snapshot_fixture(temp.path()).await?;
        game.id = "skyrim-se".into();
        sqlx::query("UPDATE profiles SET game_id=?,save_mode='profile'")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE mods SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        let prefix = temp.path().join("prefix");
        game.wine_prefix = Some(prefix.clone());
        let live =
            prefix.join("drive_c/users/test/Documents/My Games/Skyrim Special Edition/Saves");
        fs::create_dir_all(&live)?;
        fs::create_dir_all(game.data_dir())?;
        fs::write(live.join("save.dat"), b"original progress")?;
        fs::write(game.data_dir().join("File.txt"), b"original file")?;
        let bank = temp.path().join("target-bank");
        fs::create_dir(&bank)?;
        fs::write(bank.join("save.dat"), b"target progress")?;
        let source = SaveSetId::Global {
            game_id: game.id.clone(),
        };
        let target = SaveSetId::Profile {
            game_id: game.id.clone(),
            profile_id: profile.clone(),
        };
        sqlx::query(
            "INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')",
        )
        .bind(&game.id)
        .execute(&tracker.pool)
        .await?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let mut tx = durable(&tracker).await?;
        let previous = state::read(&mut tx, &game.id).await?;
        tx.rollback().await?;
        let (manifest, files, mut journal) = unpublished(&history, &game, &profile).await?;
        let saves = crate::core::save_manager::activation::prepare_staged(
            &journal.id,
            source.clone(),
            target.clone(),
            &live,
            &bank,
        )?;
        journal.attach_saves(&game, saves)?;
        journal
            .persist(&history, &game, "deploy", manifest.objects())
            .await?;
        assert!(
            super::journal::discard_save_preparation(&history, &game, &journal.id)
                .await
                .is_err()
        );
        let applied = journal.apply(&history, &game, Control::default()).await?;
        if !committed {
            sqlx::query("CREATE TRIGGER reject_activation BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected commit failure'); END").execute(&tracker.pool).await?;
        }
        let result = applied
            .commit(
                &history,
                &game,
                previous.as_ref(),
                Some(&Deployment {
                    manifest: &manifest,
                    profile: &profile,
                    files: &files,
                }),
                &target,
            )
            .await;
        assert_eq!(result.is_ok(), committed);
        let document: String =
            sqlx::query_scalar("SELECT document FROM generation_journals WHERE id=?")
                .bind(&journal.id)
                .fetch_one(&tracker.pool)
                .await?;
        drop(history);
        let history = History::open(&tracker, &game.id, temp.path(), false).await?;
        let recovered: Journal = serde_json::from_str(&document)?;
        let new_prefix = temp.path().join("reselected-prefix");
        fs::rename(&prefix, &new_prefix)?;
        game.wine_prefix = Some(new_prefix);
        assert_eq!(recovered.id, journal.id);
        super::recovery::recover_journal(&history, &game).await?;
        let current_live = crate::core::game::detect_save_dir(&game).expect("known save location");
        assert_eq!(
            fs::read(current_live.join("save.dat"))?,
            if committed {
                b"target progress".as_slice()
            } else {
                b"original progress".as_slice()
            }
        );
        assert_eq!(
            fs::read(game.data_dir().join("File.txt"))?,
            if committed {
                b"winner".as_slice()
            } else {
                b"original file".as_slice()
            }
        );
        let mut tx = durable(&tracker).await?;
        let state = state::read(&mut tx, &game.id).await?.expect("ownership");
        tx.rollback().await?;
        assert_eq!(state.saves, if committed { target } else { source });
    }
    Ok(())
}
