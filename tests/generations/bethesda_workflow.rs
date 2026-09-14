use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;

use anyhow::{Context, Result};

use super::super::{catalog::durable, manifest, prepared, state};
use super::tests::Fixture;
use super::*;

async fn draft(fixture: &Fixture, profile: &str, reuse: bool) -> Result<preparation::Prepared> {
    let mut manifest = manifest::capture(
        &fixture.history,
        &fixture.game,
        profile,
        fixture.history.cache.clone(),
        Control::default(),
    )
    .await?;
    manifest.outputs = prepared::files(&manifest)?;
    let desired = manifest
        .outputs
        .iter()
        .map(|output| {
            (
                output.target.clone(),
                match &output.content {
                    Some(identity) => Node::File {
                        identity: identity.clone(),
                        mode: output.mode,
                    },
                    None => Node::Directory { mode: output.mode },
                },
            )
        })
        .collect();
    let journal =
        Journal::prepare(&fixture.history, &fixture.game, desired, Control::default()).await?;
    preparation::prepare(
        &fixture.history,
        &fixture.game,
        manifest,
        journal,
        reuse,
        Control::default(),
    )
    .await
}

async fn publish(fixture: &Fixture, manifest: &Manifest) -> Result<String> {
    let mut tx = durable(&fixture.history.tracker).await?;
    let id = fixture.history.publish(&mut tx, manifest).await?;
    tx.commit().await?;
    Ok(id)
}

async fn apply_uncommitted(fixture: &Fixture, journal: &Journal) -> Result<()> {
    journal
        .verify_prepared(&fixture.history, &fixture.game, vec![], Control::default())
        .await?;
    journal
        .persist(&fixture.history, &fixture.game, "deploy", BTreeMap::new())
        .await?;
    journal
        .apply(&fixture.history, &fixture.game, Control::default())
        .await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn creates_missing_configuration_and_mod_directories_and_rolls_them_back() -> Result<()> {
    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    assert!(!fixture.game.data_dir().exists());
    assert!(!game::custom_ini_paths(&fixture.game)[0].exists());
    apply_uncommitted(&fixture, &prepared.journal).await?;
    assert_eq!(
        fs::read(fixture.game.data_dir().join("Test.esp"))?,
        b"winner"
    );
    for path in game::custom_ini_paths(&fixture.game) {
        assert!(path.is_file());
    }
    prepared
        .journal
        .recover(&fixture.history, &fixture.game, false)
        .await?;
    assert!(!fixture.game.data_dir().exists());
    for path in game::custom_ini_paths(&fixture.game) {
        assert!(!path.parent().context("INI parent")?.exists());
    }
    assert!(
        fixture
            .game
            .wine_prefix
            .as_ref()
            .context("prefix")?
            .join("drive_c/users/player")
            .is_dir()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn directory_recovery_preserves_external_files_and_resumes_after_resolution() -> Result<()> {
    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    apply_uncommitted(&fixture, &prepared.journal).await?;
    let path = game::custom_ini_paths(&fixture.game)[0]
        .parent()
        .context("INI parent")?
        .join("external.txt");
    fs::write(&path, b"user data")?;
    assert!(
        prepared
            .journal
            .recover(&fixture.history, &fixture.game, false)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&path)?, b"user data");
    assert!(!prepared.journal.decision(&fixture.history).await?);
    fs::remove_file(path)?;
    super::super::recovery::recover_journal(&fixture.history, &fixture.game).await?;
    assert!(!fixture.game.data_dir().exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn ini_bridges_preserve_links_and_restore_their_backing_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    let paths = game::custom_ini_paths(&fixture.game);
    for path in &paths {
        fs::create_dir_all(path.parent().context("INI parent")?)?;
    }
    fs::write(&paths[1], b"[Display]\nWidth=2560\n")?;
    symlink(&paths[1], &paths[0])?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    let outputs = prepared
        .manifest
        .outputs
        .iter()
        .filter(|output| matches!(output.target, Target::CustomIni { .. }))
        .collect::<Vec<_>>();
    assert_eq!(outputs[0].content, outputs[1].content);
    apply_uncommitted(&fixture, &prepared.journal).await?;
    assert_eq!(fs::read_link(&paths[0])?, paths[1]);
    assert!(fs::read(&paths[0])?.starts_with(b"[Display]\nWidth=2560\n"));
    assert_eq!(
        super::super::divergence::live(
            &fixture.game,
            &Target::CustomIni { slot: 0 },
            &Control::default()
        )?,
        super::super::divergence::live(
            &fixture.game,
            &Target::CustomIni { slot: 1 },
            &Control::default()
        )?
    );
    prepared
        .journal
        .recover(&fixture.history, &fixture.game, false)
        .await?;
    assert_eq!(fs::read(&paths[1])?, b"[Display]\nWidth=2560\n");
    assert_eq!(fs::read_link(&paths[0])?, paths[1]);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn changed_ini_bridges_block_application_and_recovery_without_following_them() -> Result<()> {
    let fixture = Fixture::new().await?;
    let paths = game::custom_ini_paths(&fixture.game);
    for path in &paths {
        fs::create_dir_all(path.parent().context("INI parent")?)?;
    }
    fs::write(&paths[1], b"[Display]\nWidth=1280\n")?;
    symlink(&paths[1], &paths[0])?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    apply_uncommitted(&fixture, &prepared.journal).await?;
    let outside = fixture.history.cache.join("other.ini");
    fs::write(&outside, b"external")?;
    fs::remove_file(&paths[0])?;
    symlink(&outside, &paths[0])?;
    assert!(
        prepared
            .journal
            .recover(&fixture.history, &fixture.game, false)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&outside)?, b"external");
    fs::remove_file(&paths[0])?;
    symlink(&paths[1], &paths[0])?;
    prepared
        .journal
        .recover(&fixture.history, &fixture.game, false)
        .await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn complete_bethesda_configuration_commits_and_purge_retains_history() -> Result<()> {
    use crate::core::save_manager::SaveSetId;

    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    let previous = state::State {
        generation: None,
        profile: None,
        saves: SaveSetId::Global {
            game_id: fixture.game.id.clone(),
        },
        modified: false,
    };
    sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')")
        .bind(&fixture.game.id)
        .execute(&fixture.history.tracker.pool)
        .await?;
    super::super::coordinator::activate(
        &fixture.history,
        &fixture.game,
        &prepared.journal,
        Some(&previous),
        Some(&state::Deployment {
            manifest: &prepared.manifest,
            profile: &fixture.profile,
            files: &prepared.files,
        }),
        &previous.saves,
        Control::default(),
    )
    .await?;
    let mut tx = durable(&fixture.history.tracker).await?;
    let deployed = state::read(&mut tx, &fixture.game.id)
        .await?
        .context("deployed state")?;
    tx.rollback().await?;
    assert_eq!(deployed.generation, Some(prepared.manifest.id()?));
    let desired = prepared
        .manifest
        .outputs
        .iter()
        .filter(|output| output.mod_id.is_some())
        .map(|output| (output.target.clone(), Node::Absent))
        .collect();
    let purge =
        Journal::prepare(&fixture.history, &fixture.game, desired, Control::default()).await?;
    let purge = preparation::purge(
        &fixture.history,
        &fixture.game,
        purge,
        &prepared.manifest,
        Control::default(),
    )
    .await?;
    super::super::coordinator::activate(
        &fixture.history,
        &fixture.game,
        &purge,
        Some(&deployed),
        None,
        &previous.saves,
        Control::default(),
    )
    .await?;
    assert_eq!(
        fixture.history.load(&prepared.manifest.id()?).await?,
        prepared.manifest
    );
    assert!(!fixture.game.data_dir().join("Test.esp").exists());
    for path in game::custom_ini_paths(&fixture.game) {
        assert!(path.is_file());
    }

    Ok(())
}

// @variants: both
#[tokio::test]
async fn restored_bethesda_drafts_reuse_retained_configuration_after_library_deletion() -> Result<()>
{
    let fixture = Fixture::new().await?;
    let paths = game::custom_ini_paths(&fixture.game);
    for path in &paths {
        fs::create_dir_all(path.parent().context("INI parent")?)?;
        fs::write(path, b"[Display]\nWidth=1920\n")?;
    }
    let original = draft(&fixture, &fixture.profile, true).await?;
    let id = publish(&fixture, &original.manifest).await?;
    sqlx::query("DELETE FROM profiles")
        .execute(&fixture.history.tracker.pool)
        .await?;
    sqlx::query("DELETE FROM mods")
        .execute(&fixture.history.tracker.pool)
        .await?;
    for id in ["winner", "loser", "disabled"] {
        fs::remove_dir_all(fixture.history.cache.join(id))?;
    }
    for path in &paths {
        fs::write(path, b"[Display]\nWidth=3840\n")?;
    }
    let profile =
        super::super::restore::restore(&fixture.history, &id, "Historical", Control::default())
            .await?;
    let restored = draft(&fixture, &profile, true).await?;
    assert_eq!(restored.reused.as_deref(), Some(id.as_str()));
    for output in original
        .manifest
        .outputs
        .iter()
        .filter(|output| matches!(output.target, Target::PluginControl { .. }))
    {
        assert!(restored.manifest.outputs.contains(output));
    }
    for output in restored
        .manifest
        .outputs
        .iter()
        .filter(|output| matches!(output.target, Target::CustomIni { .. }))
    {
        let path = fixture
            .history
            .store
            .source(output.content.as_ref().context("INI content")?)?;
        assert!(fs::read(path)?.starts_with(b"[Display]\nWidth=3840\n"));
    }
    assert_eq!(
        restored.manifest.sources.len(),
        original.manifest.sources.len()
    );
    assert!(restored.files.iter().all(|file| file.mod_id != "winner"));
    for path in &paths {
        assert_eq!(fs::read(path)?, b"[Display]\nWidth=3840\n");
    }
    sqlx::query("UPDATE profile_plugins SET enabled=0 WHERE profile_id=?")
        .bind(&profile)
        .execute(&fixture.history.tracker.pool)
        .await?;
    let edited = draft(&fixture, &profile, true).await?;
    assert_eq!(edited.reused, None);
    assert_ne!(edited.manifest.outputs, restored.manifest.outputs);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn changed_historical_base_inputs_require_explicit_fresh_preparation() -> Result<()> {
    let fixture = Fixture::new().await?;
    let base = fixture.game.path.join("vanilla.dat");
    fs::write(&base, b"base one")?;
    let mut original = draft(&fixture, &fixture.profile, true).await?;
    original.manifest.base_inputs.push(Output {
        target: Target::Bethesda {
            root: true,
            path: "vanilla.dat".into(),
        },
        content: Some(
            fixture
                .history
                .retain(base.clone(), Control::default())
                .await?,
        ),
        mode: 0o644,
        mod_id: None,
    });
    let id = publish(&fixture, &original.manifest).await?;
    let profile =
        super::super::restore::restore(&fixture.history, &id, "Historical", Control::default())
            .await?;
    assert!(draft(&fixture, &profile, true).await?.reused.is_some());
    fs::write(base, b"base two")?;
    assert!(draft(&fixture, &profile, true).await.is_err());
    assert_eq!(draft(&fixture, &profile, false).await?.reused, None);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn redirected_or_newly_occupied_parent_directories_block_prepared_activation() -> Result<()> {
    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    fs::create_dir_all(fixture.game.data_dir())?;
    assert!(
        prepared
            .journal
            .verify_prepared(&fixture.history, &fixture.game, vec![], Control::default())
            .await
            .is_err()
    );
    fs::remove_dir(fixture.game.data_dir())?;
    symlink(&fixture.history.cache, fixture.game.data_dir())?;
    assert!(
        prepared
            .journal
            .verify_prepared(&fixture.history, &fixture.game, vec![], Control::default())
            .await
            .is_err()
    );
    assert!(fs::symlink_metadata(fixture.game.data_dir())?.is_symlink());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn cancelled_file_application_rolls_back_created_directories() -> Result<()> {
    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    prepared
        .journal
        .persist(
            &fixture.history,
            &fixture.game,
            "deploy",
            prepared.manifest.objects(),
        )
        .await?;
    let mut control = Control::default();
    let cancelled = control.cancelled.clone();
    control.progress = std::sync::Arc::new(move |_, _| {
        cancelled.store(true, std::sync::atomic::Ordering::Release)
    });
    assert!(
        prepared
            .journal
            .apply(&fixture.history, &fixture.game, control)
            .await
            .is_err()
    );
    super::super::recovery::recover_journal(&fixture.history, &fixture.game).await?;
    assert!(!fixture.game.data_dir().exists());
    for path in game::plugins_txt_paths(&fixture.game) {
        assert!(!path.exists());
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn layout_recovery_resolves_reauthorized_game_and_prefix_roots() -> Result<()> {
    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    let document = serde_json::to_string(&prepared.journal)?;
    assert!(!document.contains(fixture.game.path.to_str().context("game path")?));
    assert!(
        !document.contains(
            fixture
                .game
                .wine_prefix
                .as_ref()
                .context("prefix")?
                .to_str()
                .context("prefix path")?
        )
    );
    apply_uncommitted(&fixture, &prepared.journal).await?;
    let mut moved = fixture.game.clone();
    moved.path = fixture.history.cache.join("new-game-grant");
    moved.wine_prefix = Some(fixture.history.cache.join("new-prefix-grant"));
    fs::rename(&fixture.game.path, &moved.path)?;
    fs::rename(
        fixture.game.wine_prefix.as_ref().context("prefix")?,
        moved.wine_prefix.as_ref().context("prefix")?,
    )?;
    assert!(
        super::super::recovery::recover_journal(&fixture.history, &fixture.game)
            .await
            .is_err()
    );
    super::super::recovery::recover_journal(&fixture.history, &moved).await?;
    assert!(!moved.data_dir().exists());
    for path in game::custom_ini_paths(&moved) {
        assert!(!path.exists());
    }
    assert!(!fixture.game.path.exists());
    assert!(
        !fixture
            .game
            .wine_prefix
            .as_ref()
            .context("prefix")?
            .exists()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn malformed_layout_journals_cannot_claim_roots_or_other_engine_anchors() -> Result<()> {
    let fixture = Fixture::new().await?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    let original = serde_json::to_value(&prepared.journal)?;
    for levels in [0, 10000] {
        let mut changed = original.clone();
        changed["directories"][0]["levels"] = levels.into();
        let journal: Journal = serde_json::from_value(changed)?;
        assert!(journal.validate(&fixture.game).is_err());
    }
    let mut changed = original.clone();
    changed["directories"][0]["target"] = serde_json::to_value(Target::Aurora {
        root: true,
        path: "Data/Test.esp".into(),
    })?;
    let journal: Journal = serde_json::from_value(changed)?;
    assert!(journal.validate(&fixture.game).is_err());
    let mut changed = original;
    changed["version"] = 1.into();
    let journal: Journal = serde_json::from_value(changed)?;
    assert!(journal.validate(&fixture.game).is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn explicit_directory_outputs_and_missing_nested_parents_recover_together() -> Result<()> {
    let fixture = Fixture::new().await?;
    let identity = fixture
        .history
        .retain_generated(b"nested".to_vec(), Control::default())
        .await?;
    let journal = Journal::prepare(
        &fixture.history,
        &fixture.game,
        vec![
            (
                Target::Bethesda {
                    root: false,
                    path: "nested/deeper/file.txt".into(),
                },
                Node::File {
                    identity,
                    mode: 0o644,
                },
            ),
            (
                Target::Bethesda {
                    root: false,
                    path: "nested".into(),
                },
                Node::Directory { mode: 0o755 },
            ),
        ],
        Control::default(),
    )
    .await?;
    apply_uncommitted(&fixture, &journal).await?;
    assert_eq!(
        fs::read(fixture.game.data_dir().join("nested/deeper/file.txt"))?,
        b"nested"
    );
    journal
        .recover(&fixture.history, &fixture.game, false)
        .await?;
    assert!(!fixture.game.data_dir().exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn committed_ini_bridge_recovery_preserves_external_edits_and_retained_history() -> Result<()>
{
    use crate::core::save_manager::SaveSetId;

    let fixture = Fixture::new().await?;
    let paths = game::custom_ini_paths(&fixture.game);
    for path in &paths {
        fs::create_dir_all(path.parent().context("INI parent")?)?;
    }
    fs::write(&paths[1], b"[Display]\nWidth=2560\n")?;
    symlink(&paths[1], &paths[0])?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    let previous = state::State {
        generation: None,
        profile: None,
        saves: SaveSetId::Global {
            game_id: fixture.game.id.clone(),
        },
        modified: false,
    };
    sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')")
        .bind(&fixture.game.id)
        .execute(&fixture.history.tracker.pool)
        .await?;
    prepared
        .journal
        .persist(
            &fixture.history,
            &fixture.game,
            "deploy",
            prepared.manifest.objects(),
        )
        .await?;
    prepared
        .journal
        .apply(&fixture.history, &fixture.game, Control::default())
        .await?
        .commit(
            &fixture.history,
            &fixture.game,
            Some(&previous),
            Some(&state::Deployment {
                manifest: &prepared.manifest,
                profile: &fixture.profile,
                files: &prepared.files,
            }),
            &previous.saves,
        )
        .await?;
    let committed = fs::read(&paths[1])?;
    fs::write(&paths[1], b"external preferences")?;
    assert!(
        super::super::recovery::recover_journal(&fixture.history, &fixture.game)
            .await
            .is_err()
    );
    assert!(prepared.journal.decision(&fixture.history).await?);
    assert_eq!(fs::read(&paths[1])?, b"external preferences");
    assert_eq!(
        fixture.history.load(&prepared.manifest.id()?).await?,
        prepared.manifest
    );
    fs::write(&paths[1], committed)?;
    super::super::recovery::recover_journal(&fixture.history, &fixture.game).await?;
    assert_eq!(fs::read_link(&paths[0])?, paths[1]);
    assert!(fixture.game.data_dir().join("Test.esp").is_file());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn bethesda_state_commit_failure_restores_files_links_and_new_directories() -> Result<()> {
    use crate::core::save_manager::SaveSetId;

    let fixture = Fixture::new().await?;
    let paths = game::custom_ini_paths(&fixture.game);
    for path in &paths {
        fs::create_dir_all(path.parent().context("INI parent")?)?;
    }
    fs::write(&paths[1], b"[Display]\nWidth=1280\n")?;
    symlink(&paths[1], &paths[0])?;
    let prepared = draft(&fixture, &fixture.profile, true).await?;
    let previous = state::State {
        generation: None,
        profile: None,
        saves: SaveSetId::Global {
            game_id: fixture.game.id.clone(),
        },
        modified: false,
    };
    sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')")
        .bind(&fixture.game.id)
        .execute(&fixture.history.tracker.pool)
        .await?;
    sqlx::query("CREATE TRIGGER reject_activation BEFORE UPDATE ON generation_game_state BEGIN SELECT RAISE(ABORT,'injected commit failure'); END").execute(&fixture.history.tracker.pool).await?;
    assert!(
        super::super::coordinator::activate(
            &fixture.history,
            &fixture.game,
            &prepared.journal,
            Some(&previous),
            Some(&state::Deployment {
                manifest: &prepared.manifest,
                profile: &fixture.profile,
                files: &prepared.files
            }),
            &previous.saves,
            Control::default()
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(&paths[1])?, b"[Display]\nWidth=1280\n");
    assert_eq!(fs::read_link(&paths[0])?, paths[1]);
    assert!(!fixture.game.data_dir().exists());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generations")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(count, 0);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generation_journals")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}
