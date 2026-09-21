use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::journal::Node;
use super::super::manifest::{Manifest, Output};
use super::super::state_tests::unpublished;
use super::*;
use crate::models::manifest::ModFile;

struct Fixture {
    history: History,
    game: Game,
    profile: String,
    manifest: Manifest,
    files: Vec<ModFile>,
    journal: Journal,
    _temp: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, profile) = super::super::tests::snapshot_fixture(temp.path()).await?;
        fs::create_dir_all(game.data_dir())?;
        fs::write(game.data_dir().join("File.txt"), b"old content")?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let (manifest, files, journal) = unpublished(&history, &game, &profile).await?;
        Ok(Self {
            history,
            game,
            profile,
            manifest,
            files,
            journal,
            _temp: temp,
        })
    }

    async fn deploy(&self, previous: Option<&State>, control: Control) -> Result<()> {
        activate(
            &self.history,
            &self.game,
            &self.journal,
            previous,
            Some(&Deployment {
                manifest: &self.manifest,
                profile: &self.profile,
                files: &self.files,
            }),
            &SaveSetId::Global {
                game_id: self.game.id.clone(),
            },
            control,
        )
        .await
    }

    async fn state(&self) -> Result<Option<State>> {
        let mut tx = durable(&self.history.tracker).await?;
        let state = state::read(&mut tx, &self.game.id).await?;
        tx.rollback().await?;
        Ok(state)
    }

    async fn pending(&self) -> Result<i64> {
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM generation_journals WHERE game_id=?")
                .bind(&self.game.id)
                .fetch_one(&self.history.tracker.pool)
                .await?,
        )
    }

    fn live(&self) -> std::path::PathBuf {
        self.game.data_dir().join("File.txt")
    }
}

// @variants: both
#[tokio::test]
async fn coordinator_reuses_history_and_purge_preserves_retained_configuration() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    fixture.deploy(None, Control::default()).await?;
    let first = fixture.state().await?.context("Committed state")?;
    assert_eq!(first.generation, Some(fixture.manifest.id()?));
    assert_eq!(fs::read(fixture.live())?, b"winner");
    assert_eq!(fixture.pending().await?, 0);
    let (manifest, files, journal) =
        unpublished(&fixture.history, &fixture.game, &fixture.profile).await?;
    fixture.manifest = manifest;
    fixture.files = files;
    fixture.journal = journal;
    fixture.deploy(Some(&first), Control::default()).await?;
    assert_eq!(fixture.state().await?, Some(first.clone()));
    let generations: i64 = sqlx::query_scalar("SELECT count(*) FROM generations")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    let activations: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_activations")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    assert_eq!((generations, activations), (1, 2));
    let purge = Journal::prepare(
        &fixture.history,
        &fixture.game,
        vec![(
            Target::file(&fixture.game.engine, "File.txt")?,
            Node::Absent,
        )],
        Control::default(),
    )
    .await?;
    activate(
        &fixture.history,
        &fixture.game,
        &purge,
        Some(&first),
        None,
        &first.saves,
        Control::default(),
    )
    .await?;
    let purged = fixture.state().await?.context("Purged state")?;
    assert!(purged.generation.is_none() && purged.profile.is_none());
    assert_eq!(purged.saves, first.saves);
    assert!(!fixture.live().exists());
    assert!(
        fixture
            .history
            .load(first.generation.as_deref().context("Generation")?)
            .await
            .is_ok()
    );
    assert_eq!(fixture.pending().await?, 0);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_rolls_back_files_when_database_publication_fails() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("CREATE TRIGGER reject_activation BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected commit failure'); END")
        .execute(&fixture.history.tracker.pool).await?;
    let error = fixture.deploy(None, Control::default()).await.unwrap_err();
    assert!(error.to_string().contains("were restored"));
    assert_eq!(fs::read(fixture.live())?, b"old content");
    assert!(fixture.state().await?.is_none());
    assert_eq!(fixture.pending().await?, 0);
    assert!(
        fixture
            .history
            .tracker
            .get_deployed_files(&fixture.game.id)
            .await?
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_cancellation_after_file_replacement_restores_previous_content() -> Result<()> {
    let fixture = Fixture::new().await?;
    let live = fixture.live();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let control = Control {
        cancelled,
        progress: Arc::new(move |_, _| {
            if fs::read(&live).is_ok_and(|bytes| bytes == b"winner") {
                flag.store(true, Ordering::Release);
            }
        }),
        ..Control::default()
    };
    assert!(fixture.deploy(None, control).await.is_err());
    assert_eq!(fs::read(fixture.live())?, b"old content");
    assert!(fixture.state().await?.is_none());
    assert_eq!(fixture.pending().await?, 0);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_preserves_external_edits_and_pending_recovery() -> Result<()> {
    let fixture = Fixture::new().await?;
    let live = fixture.live();
    let edited = Arc::new(AtomicBool::new(false));
    let control = Control {
        progress: Arc::new(move |_, _| {
            if fs::read(&live).is_ok_and(|bytes| bytes == b"winner")
                && !edited.swap(true, Ordering::AcqRel)
            {
                fs::write(&live, b"external edit").unwrap();
            }
        }),
        ..Control::default()
    };
    let error = fixture.deploy(None, control).await.unwrap_err();
    assert!(error.to_string().contains("rollback is blocked"));
    assert_eq!(fs::read(fixture.live())?, b"external edit");
    assert!(fixture.state().await?.is_none());
    assert_eq!(fixture.pending().await?, 1);
    assert!(!fixture.journal.decision(&fixture.history).await?);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_reports_committed_cleanup_failure_without_rolling_back() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("CREATE TRIGGER reject_cleanup BEFORE DELETE ON generation_journals BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END")
        .execute(&fixture.history.tracker.pool).await?;
    let error = fixture.deploy(None, Control::default()).await.unwrap_err();
    assert!(error.to_string().contains("committed; cleanup is blocked"));
    assert_eq!(fs::read(fixture.live())?, b"winner");
    assert_eq!(
        fixture
            .state()
            .await?
            .context("Committed state")?
            .generation,
        Some(fixture.manifest.id()?)
    );
    assert_eq!(fixture.pending().await?, 1);
    assert!(fixture.journal.decision(&fixture.history).await?);
    sqlx::query("DROP TRIGGER reject_cleanup")
        .execute(&fixture.history.tracker.pool)
        .await?;
    super::super::recovery::recover_journal(&fixture.history, &fixture.game).await?;
    assert_eq!(fixture.pending().await?, 0);
    assert_eq!(fs::read(fixture.live())?, b"winner");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_refuses_stale_state_and_missing_target_coverage() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')")
        .bind(&fixture.game.id)
        .execute(&fixture.history.tracker.pool)
        .await?;
    assert!(fixture.deploy(None, Control::default()).await.is_err());
    let previous = fixture.state().await?;
    fixture
        .history
        .tracker
        .clear_deployed_files(&fixture.game.id)
        .await?;
    let mut missing = fixture.files[0].clone();
    missing.game_rel_original = "Missing.txt".into();
    missing.game_rel_lowercase = "missing.txt".into();
    fixture
        .history
        .tracker
        .record_deployed_files(&fixture.game.id, &[missing])
        .await?;
    let error = fixture
        .deploy(previous.as_ref(), Control::default())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("omits a previously managed target")
    );
    assert_eq!(fs::read(fixture.live())?, b"old content");
    assert_eq!(fixture.pending().await?, 0);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_blocks_changed_base_inputs_before_publishing_intent() -> Result<()> {
    let mut fixture = Fixture::new().await?;
    let base = fixture.game.data_dir().join("Base.txt");
    fs::write(&base, b"required base")?;
    fixture.manifest.base_inputs.push(Output {
        target: Target::file(&fixture.game.engine, "Base.txt")?,
        content: Some(super::super::content::inspect(&base, &Control::default())?),
        mode: 0o644,
        mod_id: None,
    });
    fs::write(base, b"updated base")?;
    let error = fixture.deploy(None, Control::default()).await.unwrap_err();
    assert!(error.to_string().contains("Required base inputs changed"));
    assert_eq!(fs::read(fixture.live())?, b"old content");
    assert_eq!(fixture.pending().await?, 0);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_preserves_another_pending_operation() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture
        .journal
        .persist(
            &fixture.history,
            &fixture.game,
            "deploy",
            fixture.manifest.objects(),
        )
        .await?;
    let before: String = sqlx::query_scalar("SELECT document FROM generation_journals")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    assert!(fixture.deploy(None, Control::default()).await.is_err());
    let after: String = sqlx::query_scalar("SELECT document FROM generation_journals")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(before, after);
    assert_eq!(fs::read(fixture.live())?, b"old content");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_uses_the_committed_decision_after_a_lost_acknowledgement() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture
        .journal
        .persist(
            &fixture.history,
            &fixture.game,
            "deploy",
            fixture.manifest.objects(),
        )
        .await?;
    let saves = SaveSetId::Global {
        game_id: fixture.game.id.clone(),
    };
    fixture
        .journal
        .apply(&fixture.history, &fixture.game, Control::default())
        .await?
        .commit(
            &fixture.history,
            &fixture.game,
            None,
            Some(&Deployment {
                manifest: &fixture.manifest,
                profile: &fixture.profile,
                files: &fixture.files,
            }),
            &saves,
        )
        .await?;
    let error = finish(
        &fixture.history,
        &fixture.game,
        &fixture.journal,
        Err(anyhow::anyhow!("commit acknowledgement lost")),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Activation committed and recovery completed")
    );
    assert_eq!(fs::read(fixture.live())?, b"winner");
    assert_eq!(fixture.pending().await?, 0);
    assert_eq!(
        fixture
            .state()
            .await?
            .context("Committed state")?
            .generation,
        Some(fixture.manifest.id()?)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_applies_or_rolls_back_files_and_live_saves_together() -> Result<()> {
    for reject in [false, true] {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) =
            super::super::tests::snapshot_fixture(temp.path()).await?;
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
        let bank = temp.path().join("target-bank");
        fs::create_dir_all(&live)?;
        fs::create_dir(&bank)?;
        fs::write(live.join("save.dat"), b"original progress")?;
        fs::write(bank.join("save.dat"), b"target progress")?;
        fs::create_dir_all(game.data_dir())?;
        fs::write(game.data_dir().join("File.txt"), b"old content")?;
        sqlx::query(
            "INSERT INTO generation_game_state(game_id,live_save_mode) VALUES (?,'global')",
        )
        .bind(&game.id)
        .execute(&tracker.pool)
        .await?;
        let source = SaveSetId::Global {
            game_id: game.id.clone(),
        };
        let target = SaveSetId::Profile {
            game_id: game.id.clone(),
            profile_id: profile.clone(),
        };
        let previous = State {
            generation: None,
            profile: None,
            saves: source.clone(),
            modified: false,
        };
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let (manifest, files, mut journal) = unpublished(&history, &game, &profile).await?;
        let transition = crate::core::save_manager::activation::prepare_staged(
            &journal.id,
            source.clone(),
            target.clone(),
            &live,
            &bank,
        )?;
        journal.attach_saves(&game, transition)?;
        if reject {
            sqlx::query("CREATE TRIGGER reject_activation BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected commit failure'); END")
                .execute(&tracker.pool).await?;
        }
        let result = activate(
            &history,
            &game,
            &journal,
            Some(&previous),
            Some(&Deployment {
                manifest: &manifest,
                profile: &profile,
                files: &files,
            }),
            &target,
            Control::default(),
        )
        .await;
        assert_eq!(result.is_err(), reject);
        let mut tx = durable(&tracker).await?;
        let state = state::read(&mut tx, &game.id)
            .await?
            .context("Save owner")?;
        tx.rollback().await?;
        assert_eq!(state.saves, if reject { source } else { target });
        assert_eq!(
            state.generation,
            if reject { None } else { Some(manifest.id()?) }
        );
        assert_eq!(
            fs::read(live.join("save.dat"))?,
            if reject {
                b"original progress".as_slice()
            } else {
                b"target progress".as_slice()
            }
        );
        assert_eq!(
            fs::read(game.data_dir().join("File.txt"))?,
            if reject {
                b"old content".as_slice()
            } else {
                b"winner".as_slice()
            }
        );
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(pending, 0);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_preserves_files_when_the_durable_decision_cannot_be_trusted() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture
        .journal
        .persist(
            &fixture.history,
            &fixture.game,
            "deploy",
            fixture.manifest.objects(),
        )
        .await?;
    fixture
        .journal
        .apply(&fixture.history, &fixture.game, Control::default())
        .await?;
    sqlx::query("DROP TRIGGER generation_journals_immutable")
        .execute(&fixture.history.tracker.pool)
        .await?;
    sqlx::query("UPDATE generation_journals SET document_version=99")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let error = finish(
        &fixture.history,
        &fixture.game,
        &fixture.journal,
        Err(anyhow::anyhow!("interrupted activation")),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("durable decision could not be read")
    );
    assert_eq!(fs::read(fixture.live())?, b"winner");
    assert_eq!(fixture.pending().await?, 1);
    assert!(fixture.state().await?.is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn coordinator_rejects_another_profile_and_unavailable_engine_participants() -> Result<()> {
    for case in ["profile", "mele", "shared"] {
        let mut fixture = Fixture::new().await?;
        match case {
            "profile" => {
                fixture.profile = fixture
                    .history
                    .tracker
                    .create_profile(&fixture.game.id, "Other")
                    .await?
            }
            "mele" => fixture.game.engine = GameEngine::MassEffect,
            _ => fixture.manifest.shared_revision = Some(("family".into(), "revision".into())),
        }
        assert!(fixture.deploy(None, Control::default()).await.is_err());
        assert_eq!(fs::read(fixture.live())?, b"old content");
        assert_eq!(fixture.pending().await?, 0);
        assert!(fixture.state().await?.is_none());
    }
    Ok(())
}
