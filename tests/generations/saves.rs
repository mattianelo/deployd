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

// @variants: both
#[test]
fn trilogy_deploys_switch_saves_independently_without_reinstalling_mods() -> Result<()> {
    isolated(
        "trilogy_deploys_switch_saves_independently_without_reinstalling_mods",
        |root| async move {
            use super::super::{activation, journal::Journal, manifest::Output, target::Target};
            use crate::core::game::mass_effect::{
                application::Request, baseline, generations::Snapshot, journal::Journal as Mele,
                library, package::SourceFile,
            };
            use crate::models::game::{GameConfig, GameEngine};
            use crate::models::profile::SaveMode;
            use crate::utils::location::{FolderRole, FolderSelection, SelectedLocation};
            use std::os::unix::fs::MetadataExt;

            let tracker = crate::core::tracker::Tracker::open("sqlite::memory:")
                .await?
                .tracker;
            let family = root.join("family");
            let prefix = root.join("prefix");
            let cache = root.join("cache");
            fs::create_dir_all(&cache)?;
            fs::create_dir_all(family.join("Game/Launcher"))?;
            fs::write(
                family.join("Game/Launcher/MassEffectLauncher.exe"),
                b"launcher",
            )?;
            fs::write(family.join("Game/Launcher/bink2w64.dll"), b"bink")?;
            let mut configs = Vec::new();
            let relative = "BioGame/CookedPCConsole/Engine.pcc";
            for number in 1..=3 {
                let game = Game {
                    id: format!("mass-effect-le{number}"),
                    title: format!("LE{number}"),
                    path: family.join(format!("Game/ME{number}")),
                    data_subdir: "BioGame".into(),
                    engine: GameEngine::MassEffect,
                    wine_prefix: Some(prefix.clone()),
                };
                fs::create_dir_all(game.path.join("Binaries/Win64"))?;
                fs::create_dir_all(game.path.join("BioGame/CookedPCConsole"))?;
                fs::write(
                    game.path
                        .join(format!("Binaries/Win64/MassEffect{number}.exe")),
                    b"executable",
                )?;
                fs::write(game.path.join(relative), b"original")?;
                let live = prefix.join(format!("drive_c/users/test/Documents/BioWare/Mass Effect Legendary Edition/Save/ME{number}"));
                fs::create_dir_all(&live)?;
                fs::write(live.join("save.dat"), format!("LE{number} progress"))?;
                configs.push(GameConfig {
                    game,
                    custom: true,
                    locations: vec![
                        FolderSelection {
                            role: FolderRole::Game,
                            location: SelectedLocation {
                                root: family.clone(),
                                host_hint: None,
                            },
                            relative: format!("Game/ME{number}").into(),
                        },
                        FolderSelection {
                            role: FolderRole::Prefix,
                            location: SelectedLocation {
                                root: prefix.clone(),
                                host_hint: None,
                            },
                            relative: Default::default(),
                        },
                    ],
                });
            }
            baseline::configure(&tracker, &configs, &[], std::sync::Arc::new(|_| {})).await?;
            for config in &configs {
                tracker
                    .set_setting(
                        &format!("cache_dir_{}", config.game.id),
                        cache.to_str().context("Invalid test cache")?,
                    )
                    .await?;
            }

            let first = tracker.ensure_default_profile(&configs[0].game.id).await?;
            for config in &configs {
                let game = &config.game;
                let profile = tracker
                    .get_active_profile(&game.id)
                    .await?
                    .context("Missing initial trilogy member")?
                    .id;
                let history = History::open(&tracker, &game.id, &cache, true).await?;
                let previous = super::super::ownership::initialize(&history).await?;
                let data = paths::deployd_data_dir()?;
                let mod_id = uuid::Uuid::new_v4().to_string();
                let source = cache.join(&mod_id);
                fs::create_dir_all(&source)?;
                fs::write(source.join("payload.pcc"), b"installed output")?;
                let number = game.id.chars().last().context("Missing game number")?;
                let record = serde_json::json!({"version":3,"target":format!("LE{number}"),"writable_cache":true,"package":{"id":mod_id,"source_sha256":"a".repeat(64),"archive_sha256":null,"manifest_version":"9.1","mod_version":"1.0","enabled":false,"options":[]}});
                sqlx::query(
                    "INSERT INTO mods(id,game_id,name,enabled,priority) VALUES (?,?,?,0,1)",
                )
                .bind(&mod_id)
                .bind(&game.id)
                .bind("Example")
                .execute(&tracker.pool)
                .await?;
                sqlx::query(
                    "INSERT INTO profile_mods(profile_id,mod_id,enabled,priority) VALUES (?,?,0,1)",
                )
                .bind(&profile)
                .bind(&mod_id)
                .execute(&tracker.pool)
                .await?;
                sqlx::query("INSERT INTO mele_packages(mod_id,document) VALUES (?,?)")
                    .bind(&mod_id)
                    .bind(record.to_string())
                    .execute(&tracker.pool)
                    .await?;
                sqlx::query("INSERT INTO mod_files(mod_id,game_rel_lowercase,game_rel_original,cache_path) VALUES (?,?,?,?)").bind(&mod_id).bind(relative.to_lowercase()).bind(relative).bind(source.join("payload.pcc").to_str()).execute(&tracker.pool).await?;
                let mut manifest =
                    manifest::capture(&history, game, &profile, data.clone(), Control::default())
                        .await?;
                let payload = root.join("payload");
                fs::write(&payload, b"installed output")?;
                let identity = history.retain(payload, Control::default()).await?;
                let baseline = tracker
                    .load_mele_baseline(&game.id)
                    .await?
                    .context("Missing baseline")?;
                let recipe = library::desired(&tracker, game, "INT".into(), false).await?;
                let snapshot = Snapshot {
                    recipe,
                    removals: Default::default(),
                    required: Vec::new(),
                    files: vec![SourceFile {
                        relative: relative.into(),
                        size: identity.size,
                        sha256: identity.sha256.clone(),
                    }],
                };
                let id = uuid::Uuid::new_v4().to_string();
                let engine: Mele = serde_json::from_value(
                    serde_json::json!({"version":5,"id":id,"game_id":game.id,"baseline":baseline.sha256,"previous":null,"desired":snapshot.state(id.clone(),profile.clone()),"operations":[{"path":relative,"before":{"size":8,"sha256":super::super::content::inspect(&game.path.join(relative),&Control::default())?.sha256},"after":{"size":identity.size,"sha256":identity.sha256}}],"directories":[]}),
                )?;
                let mut journal = Journal::prepare(
                    &history,
                    game,
                    vec![(
                        Target::MassEffect {
                            path: relative.into(),
                        },
                        Node::File {
                            identity: identity.clone(),
                            mode: 0o644,
                        },
                    )],
                    Control::default(),
                )
                .await?;
                journal.attach_mele(game, engine)?;
                let shared = super::super::shared::prepare(
                    &history,
                    game,
                    snapshot.recipe.clone(),
                    data,
                    Control::default(),
                )
                .await?;
                journal.attach_game_shared(&history, game, shared).await?;
                manifest.version = 2;
                manifest.mele = Some(snapshot);
                manifest.shared_revision =
                    super::super::shared::identity(journal.dependency.as_ref())?;
                manifest.outputs.push(Output {
                    target: Target::MassEffect {
                        path: relative.into(),
                    },
                    content: Some(identity),
                    mode: 0o644,
                    mod_id: None,
                });
                coordinator::activate(
                    &history,
                    game,
                    &journal,
                    Some(&previous),
                    Some(&Deployment {
                        manifest: &manifest,
                        profile: &profile,
                        files: &[],
                    }),
                    &SaveSetId::Global {
                        game_id: game.id.clone(),
                    },
                    Control::default(),
                )
                .await?;
            }
            let cloned = tracker
                .clone_profile(&first.id, "Second playthrough", &configs[0].game.id)
                .await?;
            tracker
                .set_profile_save_mode(&cloned, SaveMode::ProfileSpecific)
                .await?;
            tracker.switch_profile(&configs[0].game.id, &cloned).await?;
            for (index, config) in configs.iter().enumerate() {
                let game = &config.game;
                let profile = tracker
                    .get_active_profile(&game.id)
                    .await?
                    .context("Missing selected trilogy member")?
                    .id;
                let metadata = fs::metadata(game.path.join(relative))?;
                let request = Request {
                    game: game.clone(),
                    profile: profile.clone(),
                    language: "INT".into(),
                    purge: false,
                    repair: false,
                };
                let prepared =
                    activation::prepare_unchanged(&tracker, &cache, &request, Control::default())
                        .await?
                        .context("Unchanged mods should bypass the helper")?;
                assert_eq!(prepared.change_counts(), (0, 0, 0));
                if index == 1 {
                    sqlx::query("CREATE TRIGGER fail_second BEFORE UPDATE ON generation_game_state WHEN NEW.game_id='mass-effect-le2' BEGIN SELECT RAISE(FAIL,'injected'); END").execute(&tracker.pool).await?;
                    assert!(prepared.activate(Control::default()).await.is_err());
                    sqlx::query("DROP TRIGGER fail_second")
                        .execute(&tracker.pool)
                        .await?;
                    assert!(
                        super::super::session::live_saves(&tracker, game)
                            .await?
                            .profile_id()
                            .is_none()
                    );
                } else {
                    prepared.activate(Control::default()).await?;
                    assert_eq!(
                        super::super::session::live_saves(&tracker, game)
                            .await?
                            .profile_id(),
                        Some(profile.as_str())
                    );
                }
                let after = fs::metadata(game.path.join(relative))?;
                assert_eq!(
                    (metadata.ino(), metadata.mtime(), metadata.mtime_nsec()),
                    (after.ino(), after.mtime(), after.mtime_nsec())
                );
                if index == 0 {
                    for untouched in &configs[1..] {
                        assert!(
                            super::super::session::live_saves(&tracker, &untouched.game)
                                .await?
                                .profile_id()
                                .is_none()
                        );
                    }
                }
            }
            assert_eq!(
                super::super::session::live_saves(&tracker, &configs[0].game)
                    .await?
                    .profile_id(),
                Some(cloned.as_str())
            );
            for config in &configs {
                let game = &config.game;
                let profile = tracker
                    .get_active_profile(&game.id)
                    .await?
                    .context("Missing profile")?
                    .id;
                let target = SaveSetId::Profile {
                    game_id: game.id.clone(),
                    profile_id: profile.clone(),
                };
                if super::super::session::live_saves(&tracker, game).await? != target {
                    activation::prepare_unchanged(
                        &tracker,
                        &cache,
                        &Request {
                            game: game.clone(),
                            profile: profile.clone(),
                            language: "INT".into(),
                            purge: false,
                            repair: false,
                        },
                        Control::default(),
                    )
                    .await?
                    .context("Retry should reuse installed mods")?
                    .activate(Control::default())
                    .await?;
                }
                assert!(save_manager::last_save_sync_time(&game.id, &profile).is_some());
                let live =
                    crate::core::game::detect_save_dir(game).context("Missing live saves")?;
                let initial = fs::read(live.join("save.dat"))?;
                let siblings = configs
                    .iter()
                    .filter(|other| other.game.id != game.id)
                    .map(|other| {
                        let path = crate::core::game::detect_save_dir(&other.game)
                            .context("Missing sibling saves")?
                            .join("save.dat");
                        Ok((path.clone(), fs::read(path)?))
                    })
                    .collect::<Result<Vec<_>>>()?;
                assert!(
                    save_manager::list_backups(&game.id)
                        .await?
                        .iter()
                        .any(|backup| backup.save_set.profile_id().is_none())
                );
                fs::write(live.join("save.dat"), b"new playthrough progress")?;
                save_manager::sync_save_set(game, &target, u64::MAX).await?;
                let bank = paths::saves_root()?
                    .join(&game.id)
                    .join("sets/profiles")
                    .join(&profile)
                    .join("data/save.dat");
                assert_eq!(fs::read(&bank)?, b"new playthrough progress");
                let backups = save_manager::list_backups(&game.id).await?;
                let backup = backups
                    .iter()
                    .find(|backup| {
                        backup.save_set == target
                            && backup.trigger == save_manager::BackupTrigger::ManualSync
                    })
                    .context("Missing manual sync recovery backup")?;
                save_manager::restore_backup(game, &backup.backup_id, &target, u64::MAX).await?;
                assert_eq!(fs::read(live.join("save.dat"))?, initial);
                assert_eq!(fs::read(&bank)?, initial);
                for (path, bytes) in siblings {
                    assert_eq!(fs::read(path)?, bytes);
                }
            }
            let game = &configs[0].game;
            let profile = tracker
                .get_active_profile(&game.id)
                .await?
                .context("Missing profile")?
                .id;
            let request = Request {
                game: game.clone(),
                profile,
                language: "INT".into(),
                purge: false,
                repair: false,
            };
            let history = History::open(&tracker, &game.id, &cache, true).await?;
            let generation: String = sqlx::query_scalar(
                "SELECT deployed_generation_id FROM generation_game_state WHERE game_id=?",
            )
            .bind(&game.id)
            .fetch_one(&tracker.pool)
            .await?;
            let manifest = history.load_manifest(&generation).await?;
            let output = manifest
                .outputs
                .iter()
                .find_map(|output| output.content.as_ref())
                .context("Missing retained output")?;
            fs::remove_file(history.store.source(output)?)?;
            drop(history);
            sqlx::query("UPDATE mods SET enabled=1 WHERE game_id=?")
                .bind(&game.id)
                .execute(&tracker.pool)
                .await?;
            assert!(
                activation::prepare_unchanged(&tracker, &cache, &request, Control::default())
                    .await?
                    .is_none()
            );
            sqlx::query("UPDATE mods SET enabled=0 WHERE game_id=?")
                .bind(&game.id)
                .execute(&tracker.pool)
                .await?;
            assert!(
                activation::prepare_unchanged(&tracker, &cache, &request, Control::default())
                    .await
                    .is_err()
            );
            Ok(())
        },
    )
}
