use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;

use crate::core::save_manager::SaveSetId;

use super::super::coordinator;
use super::super::state::Deployment;

struct Fixture {
    history: History,
    game: Game,
    _temp: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, profile) = super::super::tests::snapshot_fixture(temp.path()).await?;
        fs::create_dir_all(game.data_dir())?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let (manifest, files, journal) =
            super::super::state_tests::unpublished(&history, &game, &profile).await?;
        coordinator::activate(
            &history,
            &game,
            &journal,
            None,
            Some(&Deployment {
                manifest: &manifest,
                profile: &profile,
                files: &files,
            }),
            &SaveSetId::Global {
                game_id: game.id.clone(),
            },
            Control::default(),
        )
        .await?;
        Ok(Self {
            history,
            game,
            _temp: temp,
        })
    }

    async fn state(&self) -> Result<State> {
        let mut tx = durable(&self.history.tracker).await?;
        let state = state::read(&mut tx, &self.game.id)
            .await?
            .context("Deployed state")?;
        tx.rollback().await?;
        Ok(state)
    }
}

// @variants: both
#[tokio::test]
async fn reports_live_edits_and_preserves_immutable_history_and_selected_draft() -> Result<()> {
    let fixture = Fixture::new().await?;
    let before = fixture.state().await?;
    let generation = before.generation.as_deref().context("Generation")?;
    let manifest = fixture.history.load(generation).await?;
    let draft = fixture
        .history
        .tracker
        .create_profile(&fixture.game.id, "Unrelated draft")
        .await?;
    sqlx::query("UPDATE profiles SET is_active=(id=?)")
        .bind(&draft)
        .execute(&fixture.history.tracker.pool)
        .await?;
    let live = fixture.game.data_dir().join("File.txt");
    fs::write(&live, b"tool edit")?;
    fs::write(fixture.game.data_dir().join("unmanaged.txt"), b"unmanaged")?;
    let result = inspect(&fixture.history, &fixture.game, Control::default())
        .await?
        .context("Inspection")?;
    assert_eq!(result.generation, generation);
    assert_eq!(result.differences.len(), 1);
    assert!(fixture.state().await?.modified);
    assert_eq!(fixture.history.load(generation).await?, manifest);
    assert_eq!(
        fixture
            .history
            .tracker
            .get_active_profile(&fixture.game.id)
            .await?
            .map(|p| p.id),
        Some(draft)
    );
    assert_eq!(fs::read(&live)?, b"tool edit");
    assert!(
        super::super::history::list(&fixture.history.tracker, &fixture.game.id).await?[0].modified
    );
    fs::write(&live, b"winner")?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await?
            .context("Inspection")?
            .differences
            .is_empty()
    );
    assert_eq!(fixture.state().await?, before);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn detects_missing_files_and_permission_changes_without_repairing_them() -> Result<()> {
    let fixture = Fixture::new().await?;
    let live = fixture.game.data_dir().join("File.txt");
    fs::set_permissions(&live, fs::Permissions::from_mode(0o600))?;
    let changed = inspect(&fixture.history, &fixture.game, Control::default())
        .await?
        .context("Inspection")?;
    assert_eq!(changed.differences.len(), 1);
    assert!(matches!(
        changed.differences[0].actual,
        Node::File { mode: 0o600, .. }
    ));
    fs::remove_file(&live)?;
    let missing = inspect(&fixture.history, &fixture.game, Control::default())
        .await?
        .context("Inspection")?;
    assert_eq!(missing.differences[0].actual, Node::Absent);
    assert!(!live.exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_or_cancelled_inspection_preserves_the_last_persisted_status() -> Result<()> {
    let fixture = Fixture::new().await?;
    let previous = fixture.state().await?;
    fs::write(fixture.game.data_dir().join("File.txt"), b"edited")?;
    sqlx::query("CREATE TRIGGER reject_inspection BEFORE UPDATE ON generation_game_state BEGIN SELECT RAISE(ABORT,'injected failure'); END")
        .execute(&fixture.history.tracker.pool).await?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await
            .is_err()
    );
    assert_eq!(fixture.state().await?, previous);
    sqlx::query("DROP TRIGGER reject_inspection")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let mut control = Control::default();
    let cancelled = control.cancelled.clone();
    control.progress = std::sync::Arc::new(move |_, _| {
        cancelled.store(true, std::sync::atomic::Ordering::Release);
    });
    assert!(
        inspect(&fixture.history, &fixture.game, control)
            .await
            .is_err()
    );
    assert_eq!(fixture.state().await?, previous);
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await?
            .is_some()
    );
    assert!(fixture.state().await?.modified);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn unavailable_roots_and_symlinks_do_not_report_a_clean_deployment() -> Result<()> {
    let fixture = Fixture::new().await?;
    let live = fixture.game.data_dir().join("File.txt");
    fs::write(&live, b"edited")?;
    inspect(&fixture.history, &fixture.game, Control::default()).await?;
    let previous = fixture.state().await?;
    fs::remove_file(&live)?;
    symlink(fixture.history.cache.join("winner/file.txt"), &live)?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await
            .is_err()
    );
    assert_eq!(fixture.state().await?, previous);
    assert!(fs::symlink_metadata(&live)?.is_symlink());
    let moved = fixture.history.cache.join("disconnected-game");
    fs::rename(&fixture.game.path, &moved)?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await
            .is_err()
    );
    assert_eq!(fixture.state().await?, previous);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn pending_recovery_and_missing_history_block_status_publication() -> Result<()> {
    let fixture = Fixture::new().await?;
    let previous = fixture.state().await?;
    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('pending',?,'deploy',1,'{}')")
        .bind(&fixture.game.id).execute(&fixture.history.tracker.pool).await?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await
            .is_err()
    );
    assert_eq!(fixture.state().await?, previous);
    sqlx::query("DELETE FROM generation_journals")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let manifest = fixture
        .history
        .load(previous.generation.as_deref().context("Generation")?)
        .await?;
    fs::remove_file(
        fixture
            .history
            .store
            .source(manifest.outputs[0].content.as_ref().context("Content")?)?,
    )?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await
            .is_err()
    );
    assert_eq!(fixture.state().await?, previous);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn legacy_deployments_without_generations_are_not_reported_as_verified() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("UPDATE generation_game_state SET deployed_generation_id=NULL,modified=1")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let previous = fixture.state().await?;
    assert!(
        inspect(&fixture.history, &fixture.game, Control::default())
            .await?
            .is_none()
    );
    assert_eq!(fixture.state().await?, previous);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn concurrent_deployed_state_changes_prevent_stale_inspection_publication() -> Result<()> {
    let fixture = Fixture::new().await?;
    let edited = b"changed during inspection";
    fs::write(fixture.game.data_dir().join("File.txt"), edited)?;
    let pool = fixture.history.tracker.pool.clone();
    let runtime = tokio::runtime::Handle::current();
    let mut control = Control::default();
    control.progress = std::sync::Arc::new(move |_, total| {
        if total == edited.len() as u64 {
            runtime
                .block_on(sqlx::query("UPDATE generation_game_state SET modified=1").execute(&pool))
                .unwrap();
        }
    });
    let error = inspect(&fixture.history, &fixture.game, control)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Deployed state changed"));
    assert!(fixture.state().await?.modified);
    assert_eq!(fs::read(fixture.game.data_dir().join("File.txt"))?, edited);
    Ok(())
}
