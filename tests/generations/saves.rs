use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::process::Command;

use super::super::coordinator;
use super::super::journal::Node;
use super::super::manifest;
use super::super::records::Table;
use super::super::state::Deployment;
use super::*;
use crate::models::manifest::ModFile;
use crate::utils::paths;

fn isolated<F: Future<Output = Result<()>>>(
    name: &str,
    run: impl FnOnce(PathBuf) -> F,
) -> Result<()> {
    if std::env::var("DEPLOYD_SAVE_PREPARATION_CASE").as_deref() == Ok(name) {
        let root = PathBuf::from(std::env::var("DEPLOYD_SAVE_PREPARATION_ROOT")?);
        ensure!(
            paths::deployd_data_dir()?.starts_with(&root),
            "Test storage escaped isolation"
        );
        return tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(run(root));
    }
    for snap_storage in [false, true] {
        let temp = tempfile::tempdir()?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                &format!("core::generations::saves::tests::{name}"),
                "--nocapture",
            ])
            .env("DEPLOYD_SAVE_PREPARATION_CASE", name)
            .env("DEPLOYD_SAVE_PREPARATION_ROOT", temp.path())
            .env("XDG_DATA_HOME", temp.path().join("xdg"))
            .env_remove("SNAP")
            .env_remove("SNAP_NAME")
            .env_remove("SNAP_INSTANCE_NAME")
            .env_remove("SNAP_USER_COMMON")
            .env_remove("SNAP_USER_DATA");
        if snap_storage {
            command
                .env("SNAP_USER_COMMON", temp.path().join("common"))
                .env("SNAP_USER_DATA", temp.path().join("revision"));
        }
        let result = command.output()?;
        ensure!(
            result.status.success(),
            "Isolated save test failed (Snap storage: {snap_storage}):\n{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

struct Fixture {
    history: History,
    game: Game,
    profile: String,
    live: PathBuf,
}

impl Fixture {
    async fn legacy(root: PathBuf) -> Result<Self> {
        let cache = root.join("cache");
        fs::create_dir(&cache)?;
        let (tracker, mut game, profile) = super::super::tests::snapshot_fixture(&cache).await?;
        game.id = "skyrim-se".into();
        game.wine_prefix = Some(root.join("prefix"));
        sqlx::query("UPDATE profiles SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE mods SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE profiles SET is_active=1 WHERE game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        let live =
            root.join("prefix/drive_c/users/test/Documents/My Games/Skyrim Special Edition/Saves");
        fs::create_dir_all(&live)?;
        fs::write(live.join("save.dat"), b"current progress")?;
        fs::create_dir_all(game.data_dir())?;
        let history = History::open(&tracker, &game.id, &cache, true).await?;
        Ok(Self {
            history,
            game,
            profile,
            live,
        })
    }

    async fn new(root: PathBuf) -> Result<Self> {
        let fixture = Self::legacy(root).await?;
        super::super::ownership::initialize(&fixture.history).await?;
        let (manifest, files, journal) = fixture.capture().await?;
        let prepared = prepare(
            &fixture.history,
            &fixture.game,
            journal,
            &manifest,
            Control::default(),
        )
        .await?;
        fixture.activate(&manifest, &files, prepared).await?;
        Ok(fixture)
    }

    async fn capture(&self) -> Result<(Manifest, Vec<ModFile>, Journal)> {
        let mut manifest = manifest::capture(
            &self.history,
            &self.game,
            &self.profile,
            paths::deployd_data_dir()?,
            Control::default(),
        )
        .await?;
        manifest.outputs = super::super::prepared::files(&manifest)?;
        let mut files = Vec::new();
        let mut desired = Vec::new();
        for output in &manifest.outputs {
            let mod_id = output.mod_id.as_deref().context("Mod output")?;
            let row = manifest
                .records
                .iter()
                .find(|rows| rows.table == Table::Files)
                .context("Files")?
                .rows
                .iter()
                .find(|row| text(row, "mod_id").is_ok_and(|id| id == mod_id))
                .context("Mod file")?;
            files.push(ModFile {
                mod_id: mod_id.to_owned(),
                game_rel_lowercase: text(row, "game_rel_lowercase")?.to_owned(),
                game_rel_original: text(row, "game_rel_original")?.to_owned(),
                cache_path: self
                    .history
                    .cache
                    .join(
                        text(row, "cache_path")?
                            .strip_prefix("cache/")
                            .context("Cache anchor")?,
                    )
                    .to_string_lossy()
                    .into_owned(),
            });
            desired.push((
                output.target.clone(),
                Node::File {
                    identity: output.content.clone().context("File content")?,
                    mode: output.mode,
                },
            ));
        }
        let journal =
            Journal::prepare(&self.history, &self.game, desired, Control::default()).await?;
        Ok((manifest, files, journal))
    }

    async fn activate(
        &self,
        manifest: &Manifest,
        files: &[ModFile],
        prepared: Prepared,
    ) -> Result<()> {
        coordinator::activate(
            &self.history,
            &self.game,
            &prepared.journal,
            Some(&prepared.previous),
            Some(&Deployment {
                manifest,
                profile: &self.profile,
                files,
            }),
            &prepared.target,
            Control::default(),
        )
        .await
    }

    async fn state(&self) -> Result<State> {
        let mut tx = durable(&self.history.tracker).await?;
        let state = state::read(&mut tx, &self.game.id)
            .await?
            .context("Save owner")?;
        tx.rollback().await?;
        Ok(state)
    }

    fn bank(&self, set: &SaveSetId) -> Result<PathBuf> {
        let root = paths::saves_root()?.join(&self.game.id).join("sets");
        Ok(match set.profile_id() {
            Some(profile) => root.join("profiles").join(profile),
            None => root.join("global"),
        })
    }
}

// @variants: both
#[test]
fn restored_save_preparation_reseeds_until_the_first_committed_activation() -> Result<()> {
    isolated(
        "restored_save_preparation_reseeds_until_the_first_committed_activation",
        async |root| {
            let mut fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            fixture.profile = super::super::restore::restore(
                &fixture.history,
                previous.generation.as_deref().context("Generation")?,
                "Restored",
                Control::default(),
            )
            .await?;
            let target = SaveSetId::Profile {
                game_id: fixture.game.id.clone(),
                profile_id: fixture.profile.clone(),
            };
            assert!(!fixture.bank(&target)?.exists());
            let (manifest, files, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            assert_eq!(prepared.previous, previous);
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(
                fs::read(fixture.bank(&target)?.join("data/save.dat"))?,
                b"current progress"
            );
            assert_eq!(fixture.state().await?, previous);
            super::super::journal::discard_save_preparation(
                &fixture.history,
                &fixture.game,
                &prepared.journal.id,
            )
            .await?;
            fs::write(fixture.live.join("save.dat"), b"later progress")?;
            let (_, _, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            fixture.activate(&manifest, &files, prepared).await?;
            assert_eq!(fixture.state().await?.saves, target);
            assert_eq!(fs::read(fixture.live.join("save.dat"))?, b"later progress");
            let seed: bool = sqlx::query_scalar(
                "SELECT seed_live_saves FROM generation_drafts WHERE profile_id=?",
            )
            .bind(&fixture.profile)
            .fetch_one(&fixture.history.tracker.pool)
            .await?;
            assert!(!seed);
            let save = super::super::content::inspect(
                &fixture.live.join("save.dat"),
                &Control::default(),
            )?;
            let retained: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM generation_objects WHERE sha256=?)",
            )
            .bind(save.sha256)
            .fetch_one(&fixture.history.tracker.pool)
            .await?;
            assert!(!retained);
            assert!(
                !save_manager::list_backups(&fixture.game.id)
                    .await?
                    .is_empty()
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn ordinary_save_banks_switch_only_when_the_prepared_activation_commits() -> Result<()> {
    isolated(
        "ordinary_save_banks_switch_only_when_the_prepared_activation_commits",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            let target = SaveSetId::Profile {
                game_id: fixture.game.id.clone(),
                profile_id: fixture.profile.clone(),
            };
            fs::write(fixture.live.join("save.dat"), b"stored target progress")?;
            save_manager::initialize_save_set(&fixture.game, &target).await?;
            fs::write(fixture.live.join("save.dat"), b"current progress")?;
            sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
                .bind(&fixture.profile)
                .execute(&fixture.history.tracker.pool)
                .await?;
            let (manifest, files, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(fixture.state().await?, previous);
            assert_eq!(
                fs::read(fixture.bank(&previous.saves)?.join("data/save.dat"))?,
                b"current progress"
            );
            fixture.activate(&manifest, &files, prepared).await?;
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"stored target progress"
            );
            assert_eq!(fixture.state().await?.saves, target);
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn unchanged_save_ownership_does_not_create_banks_or_backups() -> Result<()> {
    isolated(
        "unchanged_save_ownership_does_not_create_banks_or_backups",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            let (manifest, _, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            assert!(!prepared.journal.has_saves());
            assert_eq!(prepared.target, previous.saves);
            assert_eq!(fixture.state().await?, previous);
            assert!(!paths::saves_root()?.exists());
            assert!(
                save_manager::list_backups(&fixture.game.id)
                    .await?
                    .is_empty()
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn save_preparation_rejects_stale_modes_and_uninitialized_ownership() -> Result<()> {
    isolated(
        "save_preparation_rejects_stale_modes_and_uninitialized_ownership",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let (manifest, _, journal) = fixture.capture().await?;
            sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
                .bind(&fixture.profile)
                .execute(&fixture.history.tracker.pool)
                .await?;
            assert!(
                prepare(
                    &fixture.history,
                    &fixture.game,
                    journal.clone(),
                    &manifest,
                    Control::default()
                )
                .await
                .is_err()
            );
            sqlx::query("DELETE FROM generation_game_state WHERE game_id=?")
                .bind(&fixture.game.id)
                .execute(&fixture.history.tracker.pool)
                .await?;
            assert!(
                prepare(
                    &fixture.history,
                    &fixture.game,
                    journal,
                    &manifest,
                    Control::default()
                )
                .await
                .is_err()
            );
            assert!(!paths::saves_root()?.exists());
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn save_preparation_preserves_pending_legacy_recovery() -> Result<()> {
    isolated(
        "save_preparation_preserves_pending_legacy_recovery",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let (manifest, _, journal) = fixture.capture().await?;
            let recovery = paths::saves_root()?
                .join(&fixture.game.id)
                .join("transition.json");
            fs::create_dir_all(recovery.parent().context("Recovery parent")?)?;
            fs::write(&recovery, b"pending legacy recovery")?;
            assert!(
                prepare(
                    &fixture.history,
                    &fixture.game,
                    journal,
                    &manifest,
                    Control::default()
                )
                .await
                .is_err()
            );
            assert_eq!(fs::read(recovery)?, b"pending legacy recovery");
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert!(
                save_manager::list_backups(&fixture.game.id)
                    .await?
                    .is_empty()
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn cancelled_save_preparation_preserves_live_saves_and_restored_initialization() -> Result<()> {
    isolated(
        "cancelled_save_preparation_preserves_live_saves_and_restored_initialization",
        async |root| {
            let mut fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            fixture.profile = super::super::restore::restore(
                &fixture.history,
                previous.generation.as_deref().context("Generation")?,
                "Restored",
                Control::default(),
            )
            .await?;
            let (manifest, files, journal) = fixture.capture().await?;
            let mut control = Control::default();
            let cancelled = control.cancelled.clone();
            control.progress = std::sync::Arc::new(move |_, _| {
                cancelled.store(true, std::sync::atomic::Ordering::Release);
            });
            assert!(
                prepare(&fixture.history, &fixture.game, journal, &manifest, control)
                    .await
                    .is_err()
            );
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(fixture.state().await?, previous);
            let seed: bool = sqlx::query_scalar(
                "SELECT seed_live_saves FROM generation_drafts WHERE profile_id=?",
            )
            .bind(&fixture.profile)
            .fetch_one(&fixture.history.tracker.pool)
            .await?;
            assert!(seed);
            let (_, _, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            fixture.activate(&manifest, &files, prepared).await?;
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn save_preparation_blocks_when_prefix_access_is_lost() -> Result<()> {
    isolated(
        "save_preparation_blocks_when_prefix_access_is_lost",
        async |root| {
            let fixture = Fixture::new(root.clone()).await?;
            let previous = fixture.state().await?;
            let (manifest, _, journal) = fixture.capture().await?;
            let mut game = fixture.game.clone();
            game.wine_prefix = None;
            assert!(
                prepare(
                    &fixture.history,
                    &game,
                    journal.clone(),
                    &manifest,
                    Control::default()
                )
                .await
                .is_err()
            );
            game.wine_prefix = Some(root.join("unavailable-prefix"));
            assert!(
                prepare(
                    &fixture.history,
                    &game,
                    journal,
                    &manifest,
                    Control::default()
                )
                .await
                .is_err()
            );
            assert_eq!(fixture.state().await?, previous);
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert!(!paths::saves_root()?.exists());
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn legacy_ownership_initialization_preserves_live_saves_and_pending_recovery() -> Result<()> {
    isolated(
        "legacy_ownership_initialization_preserves_live_saves_and_pending_recovery",
        async |root| {
            let fixture = Fixture::legacy(root).await?;
            let legacy = paths::saves_root()?
                .join(&fixture.game.id)
                .join("transition.json");
            fs::create_dir_all(legacy.parent().context("Recovery parent")?)?;
            fs::write(&legacy, b"pending legacy recovery")?;
            assert!(
                super::super::ownership::initialize(&fixture.history)
                    .await
                    .is_err()
            );
            assert_eq!(fs::read(&legacy)?, b"pending legacy recovery");
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generation_game_state")
                .fetch_one(&fixture.history.tracker.pool)
                .await?;
            assert_eq!(count, 0);
            fs::remove_file(&legacy)?;
            let state = super::super::ownership::initialize(&fixture.history).await?;
            assert_eq!(state.generation, None);
            assert_eq!(
                state.saves,
                SaveSetId::Global {
                    game_id: fixture.game.id.clone()
                }
            );
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert!(!fixture.bank(&state.saves)?.exists());
            assert!(
                save_manager::list_backups(&fixture.game.id)
                    .await?
                    .is_empty()
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn restored_drafts_cannot_invent_legacy_live_save_ownership() -> Result<()> {
    isolated(
        "restored_drafts_cannot_invent_legacy_live_save_ownership",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            let restored = super::super::restore::restore(
                &fixture.history,
                previous.generation.as_deref().context("Generation")?,
                "Restored",
                Control::default(),
            )
            .await?;
            sqlx::query("UPDATE profiles SET is_active=(id=?)")
                .bind(&restored)
                .execute(&fixture.history.tracker.pool)
                .await?;
            assert_eq!(
                super::super::ownership::initialize(&fixture.history).await?,
                previous
            );
            sqlx::query("DELETE FROM generation_game_state")
                .execute(&fixture.history.tracker.pool)
                .await?;
            assert!(
                super::super::ownership::initialize(&fixture.history)
                    .await
                    .is_err()
            );
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generation_game_state")
                .fetch_one(&fixture.history.tracker.pool)
                .await?;
            assert_eq!(count, 0);
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            let seed: bool = sqlx::query_scalar(
                "SELECT seed_live_saves FROM generation_drafts WHERE profile_id=?",
            )
            .bind(restored)
            .fetch_one(&fixture.history.tracker.pool)
            .await?;
            assert!(seed);
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn first_local_save_deploy_seeds_commits_syncs_and_returns_to_global() -> Result<()> {
    isolated(
        "first_local_save_deploy_seeds_commits_syncs_and_returns_to_global",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
                .bind(&fixture.profile)
                .execute(&fixture.history.tracker.pool)
                .await?;
            let target = SaveSetId::Profile {
                game_id: fixture.game.id.clone(),
                profile_id: fixture.profile.clone(),
            };
            assert!(!fixture.bank(&target)?.exists());
            let (manifest, files, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            assert_eq!(fixture.state().await?, previous);
            fixture.activate(&manifest, &files, prepared).await?;
            assert_eq!(fixture.state().await?.saves, target);
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(
                super::super::session::live_saves(&fixture.history.tracker, &fixture.game).await?,
                target
            );
            fs::write(fixture.live.join("save.dat"), b"new local progress")?;
            save_manager::sync_save_set(&fixture.game, &target, u64::MAX).await?;
            assert_eq!(
                fs::read(fixture.bank(&target)?.join("data/save.dat"))?,
                b"new local progress"
            );
            sqlx::query("UPDATE profiles SET save_mode='global' WHERE id=?")
                .bind(&fixture.profile)
                .execute(&fixture.history.tracker.pool)
                .await?;
            let (manifest, files, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            fixture.activate(&manifest, &files, prepared).await?;
            assert_eq!(fixture.state().await?.saves, previous.saves);
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            assert_eq!(
                fs::read(fixture.bank(&target)?.join("data/save.dat"))?,
                b"new local progress"
            );
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn first_local_save_commit_failure_preserves_global_ownership_and_live_saves() -> Result<()> {
    isolated(
        "first_local_save_commit_failure_preserves_global_ownership_and_live_saves",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
                .bind(&fixture.profile)
                .execute(&fixture.history.tracker.pool)
                .await?;
            let (manifest, files, journal) = fixture.capture().await?;
            let prepared = prepare(
                &fixture.history,
                &fixture.game,
                journal,
                &manifest,
                Control::default(),
            )
            .await?;
            sqlx::query("CREATE TRIGGER reject_activation BEFORE INSERT ON generation_activations BEGIN SELECT RAISE(ABORT,'injected commit failure'); END").execute(&fixture.history.tracker.pool).await?;
            assert!(fixture.activate(&manifest, &files, prepared).await.is_err());
            assert_eq!(fixture.state().await?, previous);
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_journals")
                .fetch_one(&fixture.history.tracker.pool)
                .await?;
            assert_eq!(pending, 0);
            Ok(())
        },
    )
}

// @variants: both
#[test]
fn incomplete_local_bank_is_preserved_instead_of_reseeded() -> Result<()> {
    isolated(
        "incomplete_local_bank_is_preserved_instead_of_reseeded",
        async |root| {
            let fixture = Fixture::new(root).await?;
            let previous = fixture.state().await?;
            let target = SaveSetId::Profile {
                game_id: fixture.game.id.clone(),
                profile_id: fixture.profile.clone(),
            };
            let bank = fixture.bank(&target)?;
            fs::create_dir_all(bank.join("data"))?;
            fs::write(bank.join("data/valuable.dat"), b"preserve this")?;
            sqlx::query("UPDATE profiles SET save_mode='profile' WHERE id=?")
                .bind(&fixture.profile)
                .execute(&fixture.history.tracker.pool)
                .await?;
            let (manifest, _, journal) = fixture.capture().await?;
            assert!(
                prepare(
                    &fixture.history,
                    &fixture.game,
                    journal,
                    &manifest,
                    Control::default()
                )
                .await
                .is_err()
            );
            assert_eq!(fixture.state().await?, previous);
            assert_eq!(fs::read(bank.join("data/valuable.dat"))?, b"preserve this");
            assert_eq!(
                fs::read(fixture.live.join("save.dat"))?,
                b"current progress"
            );
            Ok(())
        },
    )
}
