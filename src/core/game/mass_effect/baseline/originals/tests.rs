use std::os::unix::fs::symlink;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tempfile::{TempDir, tempdir};

use super::*;

const ENGINE: &str = "BioGame/CookedPCConsole/Engine.pcc";
const EXE: &str = "Binaries/Win64/MassEffect1.exe";

struct Fixture {
    _temp: TempDir,
    tracker: Tracker,
    game: Game,
    data: PathBuf,
    baseline: Baseline,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempdir()?;
        let game_root = temp.path().join("game");
        fs::create_dir_all(game_root.join("BioGame/CookedPCConsole"))?;
        fs::create_dir_all(game_root.join("Binaries/Win64"))?;
        fs::write(game_root.join(ENGINE), b"original package")?;
        fs::write(game_root.join(EXE), b"original executable")?;
        let game = Game {
            id: "mass-effect-le1".into(),
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
        super::super::configure(
            &tracker,
            &[crate::models::game::GameConfig {
                game: game.clone(),
                custom: true,
                locations: Vec::new(),
            }],
            &[],
            std::sync::Arc::new(|_| {}),
        )
        .await?;
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("missing baseline")?;
        let data = temp.path().join("data");
        Ok(Self {
            _temp: temp,
            tracker,
            game,
            data,
            baseline,
        })
    }

    async fn preserve(&self, targets: &[&str]) -> Result<Preserved> {
        preserve_in(
            self.tracker.clone(),
            self.game.clone(),
            targets.iter().map(|path| (*path).into()).collect(),
            self.data.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_, _| {}),
        )
        .await
    }

    fn stored(&self, relative: &str) -> PathBuf {
        store_root(&self.data, &self.baseline).join(relative)
    }
}

// @variants: both
#[tokio::test]
async fn preserves_only_requested_files_as_independent_originals() -> Result<()> {
    let fixture = Fixture::new().await?;
    let preserved = fixture
        .preserve(&["biogame/cookedpcconsole/engine.PCC"])
        .await?;
    assert_eq!(preserved.files.len(), 1);
    assert_eq!(preserved.files[0].relative, ENGINE);
    assert_eq!(preserved.root.join(ENGINE), fixture.stored(ENGINE));
    assert_eq!(fs::read(fixture.stored(ENGINE))?, b"original package");
    assert!(!fixture.stored(EXE).exists());
    let original = fs::metadata(fixture.stored(ENGINE))?;
    let live = fs::metadata(fixture.game.path.join(ENGINE))?;
    assert_ne!(original.ino(), live.ino());
    assert_eq!(original.nlink(), 1);
    assert_eq!(original.permissions().mode() & 0o222, 0);
    assert_eq!(
        fixture.tracker.mele_originals(&fixture.game.id).await?,
        vec![(ENGINE.into(), fixture.baseline.sha256.clone())]
    );
    fs::write(fixture.game.path.join(ENGINE), b"installed mod")?;
    let reopened = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture._temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    let restored = preserve_in(
        reopened,
        fixture.game.clone(),
        vec![ENGINE.into()],
        fixture.data.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(fs::read(restored.root.join(ENGINE))?, b"original package");
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"installed mod");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn refuses_changed_live_files_without_publishing_them_as_originals() -> Result<()> {
    let fixture = Fixture::new().await?;
    fs::write(fixture.game.path.join(ENGINE), b"modified package")?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    assert!(!fixture.stored(ENGINE).exists());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"modified package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preserves_damaged_backups_and_refuses_to_replace_them() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.preserve(&[ENGINE]).await?;
    fs::set_permissions(fixture.stored(ENGINE), fs::Permissions::from_mode(0o600))?;
    fs::write(fixture.stored(ENGINE), b"damaged original")?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    assert_eq!(fs::read(fixture.stored(ENGINE))?, b"damaged original");
    assert_eq!(
        fs::read(fixture.game.path.join(ENGINE))?,
        b"original package"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn repairs_missing_copies_only_from_files_matching_the_baseline() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.preserve(&[ENGINE]).await?;
    fs::remove_file(fixture.stored(ENGINE))?;
    fs::write(fixture.game.path.join(ENGINE), b"installed mod")?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    assert!(!fixture.stored(ENGINE).exists());
    fs::write(fixture.game.path.join(ENGINE), b"original package")?;
    fixture.preserve(&[ENGINE]).await?;
    assert_eq!(fs::read(fixture.stored(ENGINE))?, b"original package");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn retries_verified_orphans_after_database_failure_without_replacing_originals() -> Result<()>
{
    let fixture = Fixture::new().await?;
    sqlx::query("CREATE TRIGGER fail_original BEFORE INSERT ON mele_originals WHEN NEW.relative_path LIKE 'BioGame/%' BEGIN SELECT RAISE(FAIL, 'injected failure'); END")
        .execute(&fixture.tracker.pool).await?;
    assert!(fixture.preserve(&[ENGINE, EXE]).await.is_err());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    assert_eq!(fs::read(fixture.stored(ENGINE))?, b"original package");
    assert_eq!(fs::read(fixture.stored(EXE))?, b"original executable");
    sqlx::query("DROP TRIGGER fail_original")
        .execute(&fixture.tracker.pool)
        .await?;
    fs::write(fixture.game.path.join(ENGINE), b"later mod")?;
    fixture.preserve(&[ENGINE, EXE]).await?;
    assert_eq!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .len(),
        2
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"later mod");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn cancellation_during_copy_discards_the_partial_file_and_ownership() -> Result<()> {
    let fixture = Fixture::new().await?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancellation = cancelled.clone();
    let result = preserve_in(
        fixture.tracker.clone(),
        fixture.game.clone(),
        vec![ENGINE.into()],
        fixture.data.clone(),
        cancelled,
        Arc::new(move |_, _| {
            cancellation.store(true, Ordering::Release);
        }),
    )
    .await;
    assert!(result.is_err());
    assert!(!fixture.stored(ENGINE).exists());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    let directory = fixture
        .stored(ENGINE)
        .parent()
        .context("no parent")?
        .to_path_buf();
    assert_eq!(fs::read_dir(directory)?.count(), 0);
    fixture.preserve(&[ENGINE]).await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn abandoned_waiters_do_not_write_when_the_mutation_lock_is_released() -> Result<()> {
    let fixture = Fixture::new().await?;
    let lease = super::super::super::mutation_lock().lock_owned().await;
    let result = tokio::time::timeout(Duration::from_millis(40), fixture.preserve(&[ENGINE])).await;
    assert!(result.is_err());
    drop(lease);
    fixture.preserve(&[EXE]).await?;
    assert!(!fixture.stored(ENGINE).exists());
    assert_eq!(
        fixture.tracker.mele_originals(&fixture.game.id).await?,
        vec![(EXE.into(), fixture.baseline.sha256.clone())]
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_unknown_duplicate_and_escaping_targets_before_writing() -> Result<()> {
    let fixture = Fixture::new().await?;
    for targets in [
        vec!["../Engine.pcc"],
        vec!["~docs~/save"],
        vec!["/etc/passwd"],
        vec!["BioGame/missing"],
        vec![ENGINE, "biogame/cookedpcconsole/engine.pcc"],
    ] {
        assert!(fixture.preserve(&targets).await.is_err());
        assert!(!fixture.data.exists());
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_linked_sources_and_backup_entries() -> Result<()> {
    let fixture = Fixture::new().await?;
    let outside = fixture._temp.path().join("outside");
    fs::write(&outside, b"original package")?;
    fs::remove_file(fixture.game.path.join(ENGINE))?;
    symlink(&outside, fixture.game.path.join(ENGINE))?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    fs::remove_file(fixture.game.path.join(ENGINE))?;
    fs::write(fixture.game.path.join(ENGINE), b"original package")?;
    symlink(&outside, fixture.stored(ENGINE))?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    fs::remove_file(fixture.stored(ENGINE))?;
    fs::hard_link(&outside, fixture.stored(ENGINE))?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    assert_eq!(fs::read(&outside)?, b"original package");
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn rejects_lost_grants_and_storage_links_without_changing_game_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    fs::create_dir(&fixture.data)?;
    symlink(&fixture.game.path, fixture.data.join("mele-originals"))?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    assert!(!fixture.game.path.join(&fixture.game.id).exists());
    fs::remove_file(fixture.data.join("mele-originals"))?;
    fs::rename(&fixture.game.path, fixture._temp.path().join("moved-game"))?;
    assert!(fixture.preserve(&[ENGINE]).await.is_err());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn complete_progress_requires_durable_ownership() -> Result<()> {
    let fixture = Fixture::new().await?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let copy = events.clone();
    let result = preserve_in(
        fixture.tracker.clone(),
        fixture.game.clone(),
        vec![ENGINE.into()],
        fixture.data.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(move |done, total| {
            copy.lock().unwrap().push((done, total));
        }),
    )
    .await?;
    assert_eq!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .len(),
        1
    );
    let events = events.lock().unwrap();
    assert_eq!(
        events.last(),
        Some(&(result.files[0].size, result.files[0].size))
    );
    assert!(
        events[..events.len() - 1]
            .iter()
            .all(|(done, total)| done < total)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn exports_discard_original_ownership_and_retain_local_copies() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.preserve(&[ENGINE]).await?;
    crate::core::migration_export::prune_export_database(&fixture.tracker.pool, &fixture.game)
        .await?;
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    assert_eq!(fs::read(fixture.stored(ENGINE))?, b"original package");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_other_engines_through_the_application_storage_entry_point() -> Result<()> {
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
            preserve(
                fixture.tracker.clone(),
                game,
                vec![ENGINE.into()],
                Arc::new(AtomicBool::new(false)),
                Arc::new(|_, _| {})
            )
            .await
            .is_err()
        );
    }
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    Ok(())
}

// @variants: snap
#[tokio::test]
async fn abandoned_copies_retain_folder_access_until_the_writer_finishes() -> Result<()> {
    let fixture = Fixture::new().await?;
    let (ready, receive) = tokio::sync::oneshot::channel();
    let ready = Mutex::new(Some(ready));
    let (release, wait) = std::sync::mpsc::channel();
    let wait = Mutex::new(wait);
    let task = tokio::spawn(preserve_in(
        fixture.tracker.clone(),
        fixture.game.clone(),
        vec![ENGINE.into()],
        fixture.data.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(move |_, _| {
            if let Some(ready) = ready.lock().unwrap().take() {
                let _ = ready.send(());
                let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(5));
            }
        }),
    ));
    tokio::time::timeout(Duration::from_secs(5), receive).await??;
    task.abort();
    assert!(task.await.is_err());
    assert!(
        crate::core::location_recovery::activity_lock()
            .try_write_owned()
            .is_err()
    );
    release.send(())?;
    let _finished = tokio::time::timeout(
        Duration::from_secs(5),
        super::super::super::mutation_lock().lock_owned(),
    )
    .await?;
    assert!(
        crate::core::location_recovery::activity_lock()
            .try_write_owned()
            .is_ok()
    );
    assert!(!fixture.stored(ENGINE).exists());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn refuses_source_changes_during_a_copy_without_publishing_partial_originals() -> Result<()> {
    let fixture = Fixture::new().await?;
    let source = fixture.game.path.join(ENGINE);
    let result = preserve_in(
        fixture.tracker.clone(),
        fixture.game.clone(),
        vec![ENGINE.into()],
        fixture.data.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(move |_, _| {
            fs::write(&source, b"modified package").unwrap();
        }),
    )
    .await;
    assert!(result.is_err());
    assert!(!fixture.stored(ENGINE).exists());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn storage_failures_and_game_overlap_cannot_publish_original_ownership() -> Result<()> {
    let fixture = Fixture::new().await?;
    fs::create_dir(&fixture.data)?;
    fs::set_permissions(&fixture.data, fs::Permissions::from_mode(0o500))?;
    let result = fixture.preserve(&[ENGINE]).await;
    fs::set_permissions(&fixture.data, fs::Permissions::from_mode(0o700))?;
    assert!(result.is_err());
    assert!(!fixture.stored(ENGINE).exists());
    let result = preserve_in(
        fixture.tracker.clone(),
        fixture.game.clone(),
        vec![ENGINE.into()],
        fixture.game.path.join("stash"),
        Arc::new(AtomicBool::new(false)),
        Arc::new(|_, _| {}),
    )
    .await;
    assert!(result.is_err());
    assert!(!fixture.game.path.join("stash").exists());
    assert!(
        fixture
            .tracker
            .mele_originals(&fixture.game.id)
            .await?
            .is_empty()
    );
    Ok(())
}
