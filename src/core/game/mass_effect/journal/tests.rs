use std::fs;
use std::os::unix::fs::symlink;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};

use super::*;

const ENGINE: &str = "BioGame/CookedPCConsole/Engine.pcc";
const DLC: &str = "BioGame/DLC/DLC_MOD_Test/CookedPCConsole/Test.pcc";

// @variants: both
#[tokio::test]
async fn removal_journals_recover_after_restart_and_preserve_external_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    let installed = fixture
        .publish(fixture.deployment(&[(DLC, b"old content")], None)?)
        .await?;
    let mut deployment = fixture.deployment(&[], Some(&installed))?;
    deployment.removals.paths.insert(DLC.into());
    deployment.removals.dlc.insert("DLC_MOD_Test".into());
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let unexpected = fixture.game.path.join(DLC);
    assert!(
        fixture
            .run(
                deployment,
                cancelled,
                Arc::new(move |done, total| {
                    if done > 0 && done < total {
                        fs::write(&unexpected, b"external content").unwrap();
                        signal.store(true, Ordering::Release);
                    }
                })
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(DLC))?, b"external content");
    let (journal, committed) = fixture
        .tracker
        .mele_journal(&fixture.game.id)
        .await?
        .context("missing journal")?;
    assert!(!committed);
    assert_eq!(journal.version, 4);
    assert_eq!(journal.desired.version, 3);
    let tracker = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    assert!(
        recover_in(tracker.clone(), fixture.game.clone(), fixture.data.clone())
            .await
            .is_err()
    );
    fs::remove_file(fixture.game.path.join(DLC))?;
    recover_in(tracker.clone(), fixture.game.clone(), fixture.data.clone()).await?;
    assert_eq!(fs::read(fixture.game.path.join(DLC))?, b"old content");
    assert_eq!(
        tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed)
    );
    assert!(tracker.mele_journal(&fixture.game.id).await?.is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn validates_removal_versions_scopes_and_legacy_records() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (mut journal, _) = fixture.pending().await?;
    let baseline = fixture
        .tracker
        .load_mele_baseline(&fixture.game.id)
        .await?
        .context("missing baseline")?;
    let encoded = serde_json::to_value(&journal)?;
    assert!(encoded["desired"].get("removals").is_none());
    let decoded: Journal = serde_json::from_value(encoded)?;
    decoded.validate(&fixture.game, &baseline)?;
    journal.desired.files.clear();
    journal.desired.removals.paths.insert(DLC.into());
    journal.desired.removals.dlc.insert("DLC_MOD_Test".into());
    assert!(state_files(&journal.desired, &baseline, Target::Le1).is_err());
    journal.desired.version = 3;
    journal.operations = operations(&baseline, None, &journal.desired, Target::Le1, &[])?;
    journal.directories.clear();
    assert!(journal.validate(&fixture.game, &baseline).is_err());
    journal.version = 4;
    journal.validate(&fixture.game, &baseline)?;
    for path in [
        ENGINE,
        "../system/file",
        "~docs~/file",
        "BioGame/DLC/DLC_MOD_Test/file.dll",
        "BioGame/DLC/DLC_OTHER/file.pcc",
    ] {
        let mut state = journal.desired.clone();
        state.removals.paths.insert(path.into());
        assert!(
            state_files(&state, &baseline, Target::Le1).is_err(),
            "{path}"
        );
    }
    let mut state = journal.desired;
    state.removals.dlc.insert("DLC_UPD_Patch01".into());
    assert!(state_files(&state, &baseline, Target::Le3).is_err());
    Ok(())
}

struct Fixture {
    temp: TempDir,
    tracker: Tracker,
    game: Game,
    data: PathBuf,
    profile: String,
}

// @variants: both
#[tokio::test]
async fn committed_removal_cleanup_resumes_without_deleting_late_external_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    let previous = fixture
        .publish(fixture.deployment(&[(DLC, b"old")], None)?)
        .await?;
    let baseline = fixture
        .tracker
        .load_mele_baseline(&fixture.game.id)
        .await?
        .context("missing baseline")?;
    let deployment = fixture.deployment(&[], Some(&previous))?;
    let desired = State {
        version: 3,
        generation: Uuid::new_v4().to_string(),
        profile: fixture.profile.clone(),
        files: Vec::new(),
        recipe: None,
        removals: super::super::removal::Removals {
            paths: BTreeSet::from([DLC.into()]),
            dlc: BTreeSet::from(["DLC_MOD_Test".into()]),
        },
    };
    let journal = Journal {
        version: 4,
        id: desired.generation.clone(),
        game_id: fixture.game.id.clone(),
        baseline: baseline.sha256.clone(),
        operations: operations(&baseline, Some(&previous), &desired, Target::Le1, &[])?,
        previous: Some(previous),
        desired,
        family: None,
        missing_components: Vec::new(),
        directories: Vec::new(),
    };
    journal.validate(&fixture.game, &baseline)?;
    let root = storage(&fixture.data, &fixture.game.id, &journal.id);
    files::stage(
        &fixture.game.path,
        &deployment.source,
        &root,
        &journal,
        None,
        &Control::recovery(),
    )?;
    fixture.tracker.begin_mele_journal(&journal).await?;
    for operation in &journal.operations {
        files::replace(
            &fixture.game.path,
            &root,
            operation,
            false,
            &Control::recovery(),
        )?;
    }
    fixture.tracker.commit_mele_journal(&journal).await?;
    let external = fixture.game.path.join("BioGame/DLC/DLC_MOD_Test/late.txt");
    fs::write(&external, b"late data")?;
    assert!(fixture.recover().await.is_err());
    assert_eq!(fs::read(&external)?, b"late data");
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .context("missing journal")?
            .1
    );
    fs::remove_file(external)?;
    fixture.recover().await?;
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    assert!(!fixture.game.path.join("BioGame/DLC/DLC_MOD_Test").exists());
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(journal.desired)
    );
    Ok(())
}

impl Fixture {
    async fn new() -> Result<Self> {
        Self::for_target(Target::Le1).await
    }

    async fn for_target(target: Target) -> Result<Self> {
        let temp = tempdir()?;
        let number = match target {
            Target::Le1 => 1,
            Target::Le2 => 2,
            Target::Le3 => 3,
        };
        let family_root = temp.path().join("family");
        let game_root = family_root.join(format!("Game/ME{number}"));
        fs::create_dir_all(game_root.join("BioGame/CookedPCConsole"))?;
        fs::create_dir_all(game_root.join("Binaries/Win64"))?;
        fs::create_dir_all(family_root.join("Game/Launcher"))?;
        fs::write(game_root.join(ENGINE), b"original package")?;
        fs::write(
            family_root.join("Game/Launcher/MassEffectLauncher.exe"),
            b"launcher",
        )?;
        fs::write(family_root.join("Game/Launcher/bink2w64.dll"), b"bink")?;
        let executable = match target {
            Target::Le1 => "MassEffect1.exe",
            Target::Le2 => "MassEffect2.exe",
            Target::Le3 => "MassEffect3.exe",
        };
        fs::write(game_root.join("Binaries/Win64").join(executable), b"exe")?;
        let game = Game {
            id: target.game_id().into(),
            title: "LE1".into(),
            path: game_root,
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        let tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("tracker.db").display()
        ))
        .await?
        .tracker;
        super::super::baseline::configure(
            &tracker,
            &[crate::models::game::GameConfig {
                game: game.clone(),
                custom: true,
                locations: vec![crate::utils::location::FolderSelection {
                    role: crate::utils::location::FolderRole::Game,
                    location: crate::utils::location::SelectedLocation {
                        root: family_root,
                        host_hint: None,
                    },
                    relative: format!("Game/ME{number}").into(),
                }],
            }],
            &[],
            std::sync::Arc::new(|_| {}),
        )
        .await?;
        let profile = tracker.ensure_default_profile(&game.id).await?.id;
        let data = temp.path().join("data");
        Ok(Self {
            temp,
            tracker,
            game,
            data,
            profile,
        })
    }

    fn deployment(
        &self,
        entries: &[(&str, &[u8])],
        previous: Option<&State>,
    ) -> Result<Deployment> {
        let source = self.temp.path().join(Uuid::new_v4().to_string());
        fs::create_dir(&source)?;
        let mut files = Vec::new();
        for (relative, content) in entries {
            let path = source.join(relative);
            fs::create_dir_all(path.parent().context("no parent")?)?;
            fs::write(path, content)?;
            files.push(SourceFile {
                relative: (*relative).into(),
                size: content.len() as u64,
                sha256: format!("{:x}", Sha256::digest(content)),
            });
        }
        Ok(Deployment {
            removals: Default::default(),
            family: None,
            repair_components: false,
            previous: previous.map(|state| state.generation.clone()),
            profile: self.profile.clone(),
            source: source.into(),
            files,
            recipe: None,
        })
    }

    async fn publish(&self, deployment: Deployment) -> Result<State> {
        self.run(
            deployment,
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_, _| {}),
        )
        .await
    }

    async fn run(
        &self,
        deployment: Deployment,
        cancel: Arc<AtomicBool>,
        progress: Progress,
    ) -> Result<State> {
        publish_in(
            self.tracker.clone(),
            self.game.clone(),
            deployment,
            self.data.clone(),
            cancel,
            progress,
        )
        .await
    }

    async fn pending(&self) -> Result<(Journal, PathBuf)> {
        let deployment =
            self.deployment(&[(ENGINE, b"merged package"), (DLC, b"DLC package")], None)?;
        let baseline = self
            .tracker
            .load_mele_baseline(&self.game.id)
            .await?
            .context("no baseline")?;
        let desired = State {
            removals: Default::default(),
            version: 1,
            generation: Uuid::new_v4().to_string(),
            profile: self.profile.clone(),
            files: deployment.files,
            recipe: None,
        };
        let operations = operations(&baseline, None, &desired, Target::Le1, &[])?;
        let journal = Journal {
            family: None,
            missing_components: Vec::new(),
            version: 1,
            id: desired.generation.clone(),
            game_id: self.game.id.clone(),
            baseline: baseline.sha256,
            previous: None,
            desired,
            directories: files::missing_directories(&self.game.path, &operations)?,
            operations,
        };
        let root = storage(&self.data, &self.game.id, &journal.id);
        files::stage(
            &self.game.path,
            &deployment.source,
            &root,
            &journal,
            None,
            &Control::recovery(),
        )?;
        self.tracker.begin_mele_journal(&journal).await?;
        Ok((journal, root))
    }

    async fn recover(&self) -> Result<()> {
        recover_in(self.tracker.clone(), self.game.clone(), self.data.clone()).await
    }
}

// @variants: both
#[tokio::test]
async fn deploys_rebuilds_and_restores_independent_originals() -> Result<()> {
    let fixture = Fixture::new().await?;
    let first = fixture
        .publish(fixture.deployment(&[(ENGINE, b"merged package"), (DLC, b"DLC package")], None)?)
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"merged package");
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(first.clone())
    );
    assert_eq!(
        fixture
            .tracker
            .get_setting(&format!("last_deployed_profile_{}", fixture.game.id))
            .await?,
        Some(fixture.profile.clone())
    );
    let second = fixture
        .publish(fixture.deployment(&[(ENGINE, b"second merge")], Some(&first))?)
        .await?;
    assert!(!fixture.game.path.join(DLC).exists());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"second merge");
    let last = fixture
        .publish(fixture.deployment(&[], Some(&second))?)
        .await?;
    assert!(last.files.is_empty());
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    let baseline = fixture
        .tracker
        .load_mele_baseline(&fixture.game.id)
        .await?
        .context("no baseline")?;
    assert_eq!(
        fs::read(
            fixture
                .data
                .join("mele-originals")
                .join(&fixture.game.id)
                .join(baseline.sha256)
                .join(ENGINE)
        )?,
        b"original package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_stale_generations_and_changed_noop_sources() -> Result<()> {
    let fixture = Fixture::new().await?;
    let first = fixture
        .publish(fixture.deployment(&[(ENGINE, b"merged package")], None)?)
        .await?;
    assert!(
        fixture
            .publish(fixture.deployment(&[], None)?)
            .await
            .is_err()
    );
    let unchanged = fixture.deployment(&[(ENGINE, b"merged package")], Some(&first))?;
    fs::write(unchanged.source.join(ENGINE), b"altered source")?;
    assert!(fixture.publish(unchanged).await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"merged package");
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(first)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn cancellation_restores_files_and_discards_new_directories() -> Result<()> {
    let fixture = Fixture::new().await?;
    let cancel = Arc::new(AtomicBool::new(false));
    let stop = cancel.clone();
    assert!(
        fixture
            .run(
                fixture.deployment(&[(ENGINE, b"merged package"), (DLC, b"DLC package")], None)?,
                cancel,
                Arc::new(move |done, _| {
                    if done == 2 {
                        stop.store(true, Ordering::Release);
                    }
                })
            )
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    assert!(!fixture.game.path.join("BioGame/DLC").exists());
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn database_failure_rolls_back_files_and_profile_marker() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("CREATE TRIGGER reject_deployment BEFORE INSERT ON mele_deployments BEGIN SELECT RAISE(ABORT, 'injected failure'); END")
        .execute(&fixture.tracker.pool).await?;
    assert!(
        fixture
            .publish(
                fixture.deployment(&[(ENGINE, b"merged package"), (DLC, b"DLC package")], None)?
            )
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    assert!(!fixture.game.path.join(DLC).exists());
    assert!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .is_none()
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    assert!(
        fixture
            .tracker
            .get_setting(&format!("last_deployed_profile_{}", fixture.game.id))
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn restart_rolls_back_pending_but_retains_committed_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (journal, root) = fixture.pending().await?;
    files::replace(
        &fixture.game.path,
        &root,
        &journal.operations[0],
        false,
        &Control::recovery(),
    )?;
    let reopened = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    recover_in(reopened, fixture.game.clone(), fixture.data.clone()).await?;
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    assert!(!root.exists());
    let (journal, root) = fixture.pending().await?;
    for operation in &journal.operations {
        files::replace(
            &fixture.game.path,
            &root,
            operation,
            false,
            &Control::recovery(),
        )?;
    }
    fixture.tracker.commit_mele_journal(&journal).await?;
    fixture.recover().await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"merged package");
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(journal.desired)
    );
    assert!(!root.exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn external_edits_block_recovery_without_losing_journal_or_content() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (journal, root) = fixture.pending().await?;
    files::replace(
        &fixture.game.path,
        &root,
        &journal.operations[0],
        false,
        &Control::recovery(),
    )?;
    fs::write(fixture.game.path.join(ENGINE), b"external edits")?;
    assert!(fixture.recover().await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"external edits");
    assert!(
        fixture
            .tracker
            .ensure_no_mele_journal(&fixture.game.id)
            .await
            .is_err()
    );
    fs::write(fixture.game.path.join(ENGINE), b"merged package")?;
    fixture.recover().await?;
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn recovery_resumes_using_the_reauthorized_game_root() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (journal, root) = fixture.pending().await?;
    files::replace(
        &fixture.game.path,
        &root,
        &journal.operations[0],
        false,
        &Control::recovery(),
    )?;
    let relocated = fixture.temp.path().join("new-grant");
    fs::rename(&fixture.game.path, &relocated)?;
    assert!(fixture.recover().await.is_err());
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_some()
    );
    let mut game = fixture.game.clone();
    game.path = relocated;
    recover_in(fixture.tracker.clone(), game.clone(), fixture.data.clone()).await?;
    assert_eq!(fs::read(game.path.join(ENGINE))?, b"original package");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn interrupted_temporary_copies_are_recovered_but_modified_ones_are_preserved() -> Result<()>
{
    let fixture = Fixture::new().await?;
    let (journal, root) = fixture.pending().await?;
    let operation = &journal.operations[0];
    let temporary = fixture
        .game
        .path
        .join(files::temporary(&root, operation, "new")?);
    fs::write(&temporary, b"foreign bytes")?;
    assert!(fixture.recover().await.is_err());
    assert_eq!(fs::read(&temporary)?, b"foreign bytes");
    fs::write(&temporary, b"merged")?;
    fixture.recover().await?;
    assert!(!temporary.exists());
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_unsafe_destinations_links_and_case_collisions() -> Result<()> {
    let fixture = Fixture::new().await?;
    for path in [
        "../system/file",
        "../launcher/file",
        "~docs~/file",
        "Data/file",
        "Mods/file",
        "Binaries/a.dll",
        "BioGame/save.pcsav",
        "BioGame/../file",
        "BioGame/test.exe",
    ] {
        assert!(destination(path).is_err(), "accepted {path}");
    }
    let mut plan = fixture.deployment(&[(ENGINE, b"merged package")], None)?;
    let mut collision = plan.files[0].clone();
    collision.relative = "BioGame/CookedPCConsole/engine.pcc".into();
    plan.files.push(collision);
    assert!(fixture.publish(plan).await.is_err());
    let outside = fixture.temp.path().join("outside");
    fs::create_dir(&outside)?;
    symlink(&outside, fixture.game.path.join("BioGame/DLC"))?;
    assert!(
        fixture
            .publish(fixture.deployment(&[(DLC, b"DLC package")], None)?)
            .await
            .is_err()
    );
    assert!(fs::read_dir(outside)?.next().is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn unknown_journal_versions_block_recovery() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (mut journal, _) = fixture.pending().await?;
    journal.version = 99;
    sqlx::query("UPDATE mele_journals SET document = ?")
        .bind(serde_json::to_string(&journal)?)
        .execute(&fixture.tracker.pool)
        .await?;
    assert!(fixture.recover().await.is_err());
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_other_engines_before_publication() -> Result<()> {
    let fixture = Fixture::new().await?;
    for engine in [
        GameEngine::Bethesda,
        GameEngine::Aurora,
        GameEngine::Eclipse,
        GameEngine::REDEngine,
    ] {
        let mut game = fixture.game.clone();
        game.engine = engine;
        assert!(
            publish(
                fixture.tracker.clone(),
                game,
                fixture.deployment(&[], None)?,
                Arc::new(AtomicBool::new(false)),
                Arc::new(|_, _| {})
            )
            .await
            .is_err()
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn abandoned_publication_holds_leases_until_rollback_finishes() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (arrived, waiting) = tokio::sync::oneshot::channel();
    let arrived = std::sync::Mutex::new(Some(arrived));
    let (release, released) = std::sync::mpsc::channel();
    let released = std::sync::Mutex::new(released);
    let progress: Progress = Arc::new(move |done, _| {
        if done == 1 {
            if let Some(sender) = arrived.lock().unwrap().take() {
                let _ = sender.send(());
            }
            let _ = released.lock().unwrap().recv();
        }
    });
    let task = tokio::spawn(publish_in(
        fixture.tracker.clone(),
        fixture.game.clone(),
        fixture.deployment(&[(ENGINE, b"merged package")], None)?,
        fixture.data.clone(),
        Arc::new(AtomicBool::new(false)),
        progress,
    ));
    tokio::time::timeout(Duration::from_secs(15), waiting).await??;
    task.abort();
    let _ = task.await;
    assert!(super::super::mutation_lock().try_lock_owned().is_err());
    assert!(
        crate::core::location_recovery::activity_lock()
            .try_write_owned()
            .is_err()
    );
    release.send(())?;
    let _finished = tokio::time::timeout(
        Duration::from_secs(15),
        super::super::mutation_lock().lock_owned(),
    )
    .await?;
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn missing_originals_cannot_be_bypassed_with_different_path_casing() -> Result<()> {
    let fixture = Fixture::new().await?;
    fs::remove_file(fixture.game.path.join(ENGINE))?;
    let plan = fixture.deployment(
        &[("BioGame/CookedPCConsole/engine.pcc", b"replacement")],
        None,
    )?;
    assert!(fixture.publish(plan).await.is_err());
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/CookedPCConsole/engine.pcc")
            .exists()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn damaged_rollback_copies_block_recovery_without_overwriting_game_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (journal, root) = fixture.pending().await?;
    files::replace(
        &fixture.game.path,
        &root,
        &journal.operations[0],
        false,
        &Control::recovery(),
    )?;
    fs::write(root.join("old").join(ENGINE), b"damaged original")?;
    assert!(fixture.recover().await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"merged package");
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_some()
    );
    fs::write(root.join("old").join(ENGINE), b"original package")?;
    fixture.recover().await?;
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn deletion_failure_after_commit_retries_cleanup_without_rollback() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("CREATE TRIGGER reject_cleanup BEFORE DELETE ON mele_journals BEGIN SELECT RAISE(ABORT, 'injected cleanup failure'); END")
        .execute(&fixture.tracker.pool).await?;
    assert!(
        fixture
            .publish(fixture.deployment(&[(ENGINE, b"merged package")], None)?)
            .await
            .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"merged package");
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .context("no journal")?
            .1
    );
    sqlx::query("DROP TRIGGER reject_cleanup")
        .execute(&fixture.tracker.pool)
        .await?;
    fixture.recover().await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"merged package");
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn publishes_content_but_rejects_official_dlc_missing_from_the_baseline() -> Result<()> {
    for target in [Target::Le2, Target::Le3] {
        let fixture = Fixture::for_target(target).await?;
        let installed = fixture
            .publish(fixture.deployment(&[(DLC, b"DLC package")], None)?)
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(DLC))?, b"DLC package");
        fixture
            .publish(fixture.deployment(&[], Some(&installed))?)
            .await?;
        assert!(!fixture.game.path.join(DLC).exists());
        assert!(
            fixture
                .publish(
                    fixture.deployment(
                        &[(
                            "BioGame/DLC/DLC_UPD_Patch01/CookedPCConsole/Test.pcc",
                            b"unexpected"
                        )],
                        fixture
                            .tracker
                            .mele_deployment(&fixture.game.id)
                            .await?
                            .as_ref(),
                    )?
                )
                .await
                .is_err()
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn refuses_database_settings_that_could_lose_a_committed_journal() -> Result<()> {
    let fixture = Fixture::new().await?;
    let mut connections = Vec::new();
    for _ in 0..4 {
        let mut connection = fixture.tracker.pool.acquire().await?;
        sqlx::query("PRAGMA synchronous = NORMAL")
            .execute(&mut *connection)
            .await?;
        connections.push(connection);
    }
    drop(connections);
    assert!(
        fixture
            .publish(fixture.deployment(&[(ENGINE, b"merged package")], None)?)
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn executable_paths_require_exact_component_claims_and_versioned_state() -> Result<()> {
    use crate::core::game::mass_effect::components;
    let fixture = Fixture::new().await?;
    let mut baseline = fixture
        .tracker
        .load_mele_baseline(&fixture.game.id)
        .await?
        .context("missing baseline")?;
    baseline.files.push(super::super::baseline::BaselineFile {
        relative: components::BINK.into(),
        size: 4,
        modified: 0,
        sha256: "a".repeat(64),
    });
    let recipe = super::super::recipe::Recipe {
        version: 2,
        backend_version: 1,
        target: Target::Le1,
        language: "INT".into(),
        helper_version: None,
        packages: Vec::new(),
        launcher: Vec::new(),
        components: components::required(Target::Le1),
    };
    let mut state = State {
        removals: Default::default(),
        version: 2,
        generation: Uuid::new_v4().to_string(),
        profile: fixture.profile.clone(),
        files: components::inventory(&recipe.components, &baseline, Target::Le1)?,
        recipe: Some(recipe),
    };
    state_files(&state, &baseline, Target::Le1)?;
    state.version = 1;
    assert!(state_files(&state, &baseline, Target::Le1).is_err());
    state.version = 2;
    let good = state.clone();
    state.files[0].sha256 = "f".repeat(64);
    assert!(state_files(&state, &baseline, Target::Le1).is_err());
    state = good.clone();
    state.files.push(SourceFile {
        relative: "Binaries/Win64/other.dll".into(),
        size: 4,
        sha256: "a".repeat(64),
    });
    assert!(state_files(&state, &baseline, Target::Le1).is_err());
    assert!(
        operations(
            &baseline,
            Some(&good),
            &good,
            Target::Le1,
            &["BioGame/CookedPCConsole/Engine.pcc".into()]
        )
        .is_err()
    );
    let repair = operations(
        &baseline,
        Some(&good),
        &good,
        Target::Le1,
        &[components::BINK.into()],
    )?;
    assert_eq!(repair.len(), 1);
    assert!(repair[0].before.is_none());
    assert!(repair[0].after.is_some());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn helper_upgrades_preserve_old_deployments_and_allow_restoration() -> Result<()> {
    let fixture = Fixture::new().await?;
    let mut deployment = fixture.deployment(&[(ENGINE, b"older merged output")], None)?;
    deployment.recipe = Some(super::super::recipe::Recipe {
        version: 1,
        backend_version: 1,
        target: Target::Le1,
        language: "INT".into(),
        helper_version: Some("0.7.1".into()),
        packages: Vec::new(),
        launcher: Vec::new(),
        components: Vec::new(),
    });
    let installed = fixture.publish(deployment).await?;
    let tracker = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    recover_in(tracker.clone(), fixture.game.clone(), fixture.data.clone()).await?;
    assert_eq!(
        tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed.clone())
    );
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"older merged output"
    );
    fixture
        .publish(fixture.deployment(&[], Some(&installed))?)
        .await?;
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    Ok(())
}
