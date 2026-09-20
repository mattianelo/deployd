use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;

use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};

use super::catalog::{History, durable};
use super::content::{self, Control};
use super::manifest;
use super::records::{self, Table};
use super::store::Store;
use super::target::Target;

// @variants: both
#[test]
fn retained_bytes_are_independent_and_deduplicated() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, b"old bytes")?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    let identity = store.retain(&source, &control)?;
    assert_eq!(identity, store.retain(&source, &control)?);
    let restored = temp.path().join("restored");
    store.materialize(&identity, &restored, 0o600, &control)?;
    fs::write(&source, b"cache changes")?;
    fs::write(&restored, b"tool changes")?;
    store.verify(&identity, &control)?;
    assert_eq!(fs::metadata(store.source(&identity)?)?.nlink(), 1);
    assert_eq!(
        fs::read_dir(temp.path().join("deployd-history/game/objects"))?.count(),
        1
    );
    Ok(())
}

// @variants: both
#[test]
fn cancelled_or_changed_reads_never_publish_objects() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, vec![5; 300_000])?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    control.cancelled.store(true, Ordering::Release);
    assert!(store.retain(&source, &control).is_err());
    let edit = source.clone();
    let control = Control {
        progress: Arc::new(move |_, _| {
            fs::write(&edit, b"changed during read").expect("fixture mutation");
        }),
        ..Control::default()
    };
    assert!(store.retain(&source, &control).is_err());
    assert_eq!(
        fs::read_dir(temp.path().join("deployd-history/game/objects"))?.count(),
        0
    );
    assert_eq!(
        fs::read_dir(temp.path().join("deployd-history/game/staging"))?.count(),
        0
    );
    Ok(())
}

// @variants: both
#[test]
fn corruption_and_missing_objects_preserve_restore_destinations() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::write(&source, b"original")?;
    fs::write(&destination, b"keep this")?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    let identity = store.retain(&source, &control)?;
    let object = store.source(&identity)?;
    fs::set_permissions(&object, fs::Permissions::from_mode(0o600))?;
    fs::write(&object, b"corrupt")?;
    assert!(
        store
            .materialize(&identity, &destination, 0o600, &control)
            .is_err()
    );
    fs::remove_file(&object)?;
    assert!(
        store
            .materialize(&identity, &destination, 0o600, &control)
            .is_err()
    );
    assert_eq!(fs::read(destination)?, b"keep this");
    Ok(())
}

// @variants: both
#[test]
fn replacement_preserves_existing_hardlink_bytes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let cached = temp.path().join("cached");
    let deployed = temp.path().join("deployed");
    fs::write(&source, b"new")?;
    fs::write(&cached, b"old")?;
    fs::hard_link(&cached, &deployed)?;
    let identity = content::inspect(&source, &Control::default())?;
    content::copy(&source, &cached, &identity, 0o600, &Control::default())?;
    assert_eq!(fs::read(cached)?, b"new");
    assert_eq!(fs::read(deployed)?, b"old");
    Ok(())
}

// @variants: both
#[test]
fn rejects_symlinked_stores_and_sources() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, b"file")?;
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&source, &link)?;
    assert!(content::inspect(&link, &Control::default()).is_err());
    let cache = temp.path().join("cache");
    fs::create_dir(&cache)?;
    std::os::unix::fs::symlink(temp.path(), cache.join("deployd-history"))?;
    assert!(Store::create(&cache, "game", "store").is_err());
    assert!(crate::utils::paths::generation_store_in(temp.path(), "../escape").is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn missing_cache_access_never_creates_a_replacement_store() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    let offline = temp.path().join("offline");
    fs::create_dir(&cache)?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    drop(History::open(&tracker, "game", &cache, true).await?);
    fs::rename(&cache, &offline)?;
    assert!(History::open(&tracker, "game", &cache, true).await.is_err());
    assert!(!cache.exists());
    fs::rename(&offline, &cache)?;
    drop(History::open(&tracker, "game", &cache, false).await?);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn store_initialization_recovers_its_durable_intent() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let cache = temp.path().join("cache");
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    assert!(History::open(&tracker, "game", &cache, true).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        1
    );
    fs::create_dir(&cache)?;
    drop(History::open(&tracker, "game", &cache, true).await?);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

pub(super) async fn snapshot_fixture(cache: &Path) -> Result<(Tracker, Game, String)> {
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let game = Game {
        id: "game".into(),
        title: "Game".into(),
        path: cache.join("game"),
        data_subdir: "Data".into(),
        engine: GameEngine::Bethesda,
        wine_prefix: None,
    };
    let profile = tracker.create_profile(&game.id, "Original").await?;
    for (id, enabled, priority) in [("loser", 1, 1), ("winner", 1, 2), ("disabled", 0, 3)] {
        fs::create_dir(cache.join(id))?;
        fs::write(cache.join(id).join("file.txt"), id)?;
        sqlx::query("INSERT INTO mods(id,game_id,name,enabled,priority) VALUES (?,'game',?,?,?)")
            .bind(id)
            .bind(id)
            .bind(enabled)
            .bind(priority)
            .execute(&tracker.pool)
            .await?;
        sqlx::query(
            "INSERT INTO profile_mods(profile_id,mod_id,enabled,priority) VALUES (?,?,?,?)",
        )
        .bind(&profile)
        .bind(id)
        .bind(enabled)
        .bind(priority)
        .execute(&tracker.pool)
        .await?;
        sqlx::query("INSERT INTO mod_files(mod_id,game_rel_lowercase,game_rel_original,cache_path) VALUES (?,'file.txt','File.txt',?)").bind(id).bind(cache.join(id).join("file.txt").to_str()).execute(&tracker.pool).await?;
    }
    Ok((tracker, game, profile))
}

// @variants: both
#[tokio::test]
async fn complete_snapshots_retain_disabled_and_losing_versions_after_library_deletion()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    assert_eq!(
        manifest
            .sources
            .iter()
            .filter(|file| file.content.is_some())
            .count(),
        3
    );
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    tx.commit().await?;
    for mod_id in ["loser", "winner", "disabled"] {
        fs::remove_dir_all(temp.path().join(mod_id))?;
        tracker.delete_mod(mod_id).await?;
    }
    sqlx::query("DELETE FROM profiles WHERE id=?")
        .bind(&profile)
        .execute(&tracker.pool)
        .await?;
    let restored = history.load(&id).await?;
    assert_eq!(restored, manifest);
    for file in restored
        .sources
        .iter()
        .filter(|file| file.content.is_some())
    {
        let target = temp.path().join("materialized");
        history
            .materialize(
                file.content.clone().expect("filtered content"),
                target.clone(),
                Control::default(),
            )
            .await?;
        assert!(!fs::read(target)?.is_empty());
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn incomplete_inventory_does_not_publish_a_generation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    fs::remove_file(temp.path().join("disabled/file.txt"))?;
    assert!(
        manifest::capture(
            &history,
            &game,
            &profile,
            temp.path().to_owned(),
            Control::default()
        )
        .await
        .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[test]
fn logical_targets_preserve_engine_anchors_and_reject_other_engines() -> Result<()> {
    let game = |engine| Game {
        id: "game".into(),
        title: "Game".into(),
        path: "game".into(),
        data_subdir: "Data".into(),
        engine,
        wine_prefix: None,
    };
    let root = Target::file(&GameEngine::Bethesda, "../engine.ini")?;
    assert_eq!(
        root.resolve(&game(GameEngine::Bethesda))?,
        Path::new("game/engine.ini")
    );
    assert!(root.resolve(&game(GameEngine::Eclipse)).is_err());
    assert!(Target::file(&GameEngine::Bethesda, "~docs~/file.ini").is_err());
    assert!(Target::file(&GameEngine::Eclipse, "../engine.ini").is_err());
    for anchor in ["system", "launcher", "register"] {
        let target = Target::file(&GameEngine::Aurora, &format!("../{anchor}/file"))?;
        assert_eq!(
            target.resolve(&game(GameEngine::Aurora))?,
            Path::new("game").join(anchor).join("file")
        );
        assert!(target.resolve(&game(GameEngine::REDEngine)).is_err());
    }
    let launcher = Target::MeleLauncher {
        path: "Content/Intro.bik".into(),
    };
    let mut mele = game(GameEngine::MassEffect);
    mele.path = "grant/Game/ME1".into();
    assert_eq!(
        launcher.resolve(&mele)?,
        Path::new("grant/Game/Launcher/Content/Intro.bik")
    );
    for engine in [
        GameEngine::Bethesda,
        GameEngine::Aurora,
        GameEngine::Eclipse,
        GameEngine::REDEngine,
    ] {
        assert!(launcher.resolve(&game(engine)).is_err());
    }
    for path in ["../system/escape", "../launcher/escape", "~docs~/file.ini"] {
        assert!(
            Target::MeleLauncher { path: path.into() }
                .resolve(&mele)
                .is_err()
        );
    }
    assert!(Target::file(&GameEngine::Bethesda, "../../escape").is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn published_metadata_is_reproducible_and_only_commits_with_its_caller() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let second = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    assert_eq!(manifest.fingerprint()?, second.fingerprint()?);
    assert_eq!(manifest.id()?, second.id()?);
    let mut tx = durable(&tracker).await?;
    history.publish(&mut tx, &manifest).await?;
    tx.rollback().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generations")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    let mods = &manifest
        .records
        .iter()
        .find(|rows| rows.table == Table::Mods)
        .expect("mod records")
        .rows;
    assert_eq!(records::text(&mods[0], "game_id")?, "game");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn unchanged_sources_reuse_retained_identities() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let first = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;

    let transferred = Arc::new(AtomicU64::new(0));
    let observed = transferred.clone();
    let control = Control {
        progress: Arc::new(move |bytes, _| {
            observed.fetch_add(bytes, Ordering::Relaxed);
        }),
        ..Control::default()
    };
    let second = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        control.clone(),
    )
    .await?;
    assert_eq!(transferred.load(Ordering::Relaxed), 0);
    assert_eq!(first.id()?, second.id()?);

    let replacement = temp.path().join("replacement");
    fs::write(&replacement, b"edited")?;
    fs::rename(replacement, temp.path().join("winner/file.txt"))?;
    let third =
        manifest::capture(&history, &game, &profile, temp.path().to_owned(), control).await?;
    assert!(transferred.load(Ordering::Relaxed) > 0);
    assert_ne!(second.id()?, third.id()?);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn uncommitted_activation_recovers_previous_files_and_permissions() -> Result<()> {
    use super::journal::{Journal, Node};
    let temp = tempfile::tempdir()?;
    let (tracker, game, _) = snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let live = game.data_dir().join("file.txt");
    fs::write(&live, b"original")?;
    fs::set_permissions(&live, fs::Permissions::from_mode(0o640))?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let identity = history
        .retain(temp.path().join("winner/file.txt"), Control::default())
        .await?;
    let journal = Journal::prepare(
        &history,
        &game,
        vec![(
            Target::file(&game.engine, "file.txt")?,
            Node::File {
                identity,
                mode: 0o644,
            },
        )],
        Control::default(),
    )
    .await?;
    journal
        .persist(&history, &game, "deploy", Default::default())
        .await?;
    let _applied = journal.apply(&history, &game, Control::default()).await?;
    assert_eq!(fs::read(&live)?, b"winner");
    journal.recover(&history, &game, false).await?;
    assert_eq!(fs::read(&live)?, b"original");
    assert_eq!(fs::metadata(&live)?.permissions().mode() & 0o777, 0o640);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn external_edits_block_rollback_without_destroying_recovery_records() -> Result<()> {
    use super::journal::{Journal, Node};
    let temp = tempfile::tempdir()?;
    let (tracker, game, _) = snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let live = game.data_dir().join("file.txt");
    fs::write(&live, b"original")?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let identity = history
        .retain(temp.path().join("winner/file.txt"), Control::default())
        .await?;
    let journal = Journal::prepare(
        &history,
        &game,
        vec![(
            Target::file(&game.engine, "file.txt")?,
            Node::File {
                identity,
                mode: 0o600,
            },
        )],
        Control::default(),
    )
    .await?;
    journal
        .persist(&history, &game, "deploy", Default::default())
        .await?;
    let _applied = journal.apply(&history, &game, Control::default()).await?;
    fs::write(&live, b"external edit")?;
    assert!(journal.recover(&history, &game, false).await.is_err());
    assert_eq!(fs::read(live)?, b"external edit");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        1
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn committed_activation_cannot_be_rolled_back_and_finishes_cleanup() -> Result<()> {
    use super::journal::{Journal, Node};
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let live = game.data_dir().join("File.txt");
    fs::write(&live, b"original")?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let mut manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let identity = history
        .retain(temp.path().join("winner/file.txt"), Control::default())
        .await?;
    let journal = Journal::prepare(
        &history,
        &game,
        vec![(
            Target::file(&game.engine, "File.txt")?,
            Node::File {
                identity,
                mode: 0o644,
            },
        )],
        Control::default(),
    )
    .await?;
    journal
        .persist(&history, &game, "deploy", manifest.objects())
        .await?;
    let applied = journal.apply(&history, &game, Control::default()).await?;
    manifest.outputs = super::prepared::files(&manifest)?;
    let files = vec![crate::models::manifest::ModFile {
        mod_id: "winner".into(),
        game_rel_lowercase: "file.txt".into(),
        game_rel_original: "File.txt".into(),
        cache_path: temp
            .path()
            .join("winner/file.txt")
            .to_string_lossy()
            .into_owned(),
    }];
    applied
        .commit(
            &history,
            &game,
            None,
            Some(&super::state::Deployment {
                manifest: &manifest,
                profile: &profile,
                files: &files,
            }),
            &crate::core::save_manager::SaveSetId::Global {
                game_id: game.id.clone(),
            },
        )
        .await?;
    assert!(journal.recover(&history, &game, false).await.is_err());
    assert_eq!(fs::read(&live)?, b"winner");
    journal.recover(&history, &game, true).await?;
    assert_eq!(fs::read(live)?, b"winner");
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
async fn interrupted_payload_deletion_replays_without_reusing_the_object() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, _) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let identity = history
        .retain(temp.path().join("winner/file.txt"), Control::default())
        .await?;
    sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES (?,?)")
        .bind(&game.id)
        .bind(&identity.sha256)
        .execute(&tracker.pool)
        .await?;
    fs::remove_file(history.store.source(&identity)?)?;
    history.finish_deletions().await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_objects")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn history_materializes_a_new_editable_profile_with_independent_identities() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    sqlx::query("INSERT INTO plugins(id,mod_id,filename,load_order,enabled) VALUES ('plugin','winner','Test.esp',7,1)").execute(&tracker.pool).await?;
    sqlx::query("INSERT INTO profile_plugins(profile_id,plugin_id,load_order,enabled) VALUES (?,'plugin',7,1)").bind(&profile).execute(&tracker.pool).await?;
    sqlx::query("INSERT INTO plugin_masters(plugin_id,master) VALUES ('plugin','Base.esm')")
        .execute(&tracker.pool)
        .await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    tx.commit().await?;
    let new = super::restore::restore(&history, &id, "Restored", Control::default()).await?;
    assert_ne!(new, profile);
    let state: (String, bool) =
        sqlx::query_as("SELECT save_mode,is_active FROM profiles WHERE id=?")
            .bind(&new)
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(state, ("profile".into(), false));
    let seeded: bool =
        sqlx::query_scalar("SELECT seed_live_saves FROM generation_drafts WHERE profile_id=?")
            .bind(&new)
            .fetch_one(&tracker.pool)
            .await?;
    assert!(seeded);
    let plugin: (String, i64) =
        sqlx::query_as("SELECT plugin_id,load_order FROM profile_plugins WHERE profile_id=?")
            .bind(&new)
            .fetch_one(&tracker.pool)
            .await?;
    assert_ne!(plugin.0, "plugin");
    assert_eq!(plugin.1, 7);
    let master: String = sqlx::query_scalar("SELECT master FROM plugin_masters WHERE plugin_id=?")
        .bind(&plugin.0)
        .fetch_one(&tracker.pool)
        .await?;
    assert_eq!(master, "Base.esm");
    let restored = manifest::capture(
        &history,
        &game,
        &new,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let fingerprint: String =
        sqlx::query_scalar("SELECT fingerprint FROM generation_drafts WHERE profile_id=?")
            .bind(&new)
            .fetch_one(&tracker.pool)
            .await?;
    assert_eq!(restored.fingerprint()?, fingerprint);
    let copies: Vec<String> = sqlx::query_scalar("SELECT mf.cache_path FROM mod_files mf JOIN profile_mods pm ON pm.mod_id=mf.mod_id WHERE pm.profile_id=?").bind(&new).fetch_all(&tracker.pool).await?;
    assert_eq!(copies.len(), 3);
    fs::write(&copies[0], b"editable")?;
    assert_eq!(history.load(&id).await?, manifest);
    assert_eq!(fs::read(temp.path().join("winner/file.txt"))?, b"winner");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_profile_commit_removes_only_its_materialized_copies() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    tx.commit().await?;
    sqlx::query("CREATE TRIGGER reject_restored_profile BEFORE INSERT ON profiles BEGIN SELECT RAISE(ABORT,'injected commit failure'); END").execute(&tracker.pool).await?;
    assert!(
        super::restore::restore(&history, &id, "Restored", Control::default())
            .await
            .is_err()
    );
    super::restore::recover(&history).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    assert_eq!(tracker.list_profiles(&game.id).await?.len(), 1);
    assert_eq!(tracker.list_mods(&game.id).await?.len(), 3);
    let cache_children = fs::read_dir(temp.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(cache_children.len(), 4);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn history_deletion_reclaims_only_unshared_bytes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let first = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let first_id = history.publish(&mut tx, &first).await?;
    tx.commit().await?;
    fs::write(temp.path().join("winner/file.txt"), b"updated winner")?;
    let second = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let second_id = history.publish(&mut tx, &second).await?;
    tx.commit().await?;
    let before = super::history::usage(&tracker, &game.id).await?;
    assert_eq!(
        super::history::deletion_size(&tracker, &game.id, &first_id).await?,
        6
    );
    super::history::delete(&history, &first_id).await?;
    assert_eq!(super::history::usage(&tracker, &game.id).await?, before - 6);
    assert!(history.load(&first_id).await.is_err());
    assert_eq!(history.load(&second_id).await?, second);
    let entries = super::history::list(&tracker, &game.id).await?;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, second_id);
    assert!(!entries[0].deployed);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn deployed_restored_and_pending_generations_cannot_be_deleted() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    tx.commit().await?;
    sqlx::query("INSERT INTO generation_game_state(game_id,deployed_generation_id,live_save_mode,modified) VALUES (?,?,'global',1)").bind(&game.id).bind(&id).execute(&tracker.pool).await?;
    assert!(super::history::delete(&history, &id).await.is_err());
    let entries = super::history::list(&tracker, &game.id).await?;
    assert!(entries[0].deployed && entries[0].modified);
    sqlx::query("DELETE FROM generation_game_state")
        .execute(&tracker.pool)
        .await?;
    sqlx::query("INSERT INTO generation_drafts(profile_id,game_id,generation_id,fingerprint,seed_live_saves) VALUES (?,?,?,?,1)").bind(&profile).bind(&game.id).bind(&id).bind(manifest.fingerprint()?).execute(&tracker.pool).await?;
    assert!(super::history::delete(&history, &id).await.is_err());
    assert_eq!(
        super::history::list(&tracker, &game.id).await?[0].restored_drafts,
        1
    );
    sqlx::query("DELETE FROM generation_drafts")
        .execute(&tracker.pool)
        .await?;
    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('pending',?,'deploy',1,'{}')").bind(&game.id).execute(&tracker.pool).await?;
    assert!(
        super::history::deletion_size(&tracker, &game.id, &id)
            .await
            .is_err()
    );
    assert!(super::history::delete(&history, &id).await.is_err());
    assert!(super::history::list(&tracker, &game.id).await?[0].recovery_pending);
    assert_eq!(history.load(&id).await?, manifest);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn deleting_game_history_preserves_shared_launcher_payloads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    sqlx::query("INSERT INTO generation_shared_revisions(family_id,id,created_at,manifest_version,manifest) VALUES ('family','revision','now',1,'{}')").execute(&mut *tx).await?;
    for (hash, _) in manifest.objects() {
        sqlx::query("INSERT INTO generation_shared_objects(family_id,revision_id,game_id,sha256) VALUES ('family','revision',?,?)").bind(&game.id).bind(hash).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let before = super::history::usage(&tracker, &game.id).await?;
    assert_eq!(
        super::history::deletion_size(&tracker, &game.id, &id).await?,
        0
    );
    super::history::delete(&history, &id).await?;
    assert_eq!(super::history::usage(&tracker, &game.id).await?, before);
    for (sha256, size) in manifest.objects() {
        history.store.verify(
            &super::content::Identity { sha256, size },
            &Control::default(),
        )?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn frozen_outputs_ignore_later_draft_edits_and_compare_content() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let first = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let outputs = super::prepared::files(&first)?;
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].mod_id.as_deref(), Some("winner"));
    fs::write(temp.path().join("winner/file.txt"), b"new version")?;
    let second = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let updated = super::prepared::files(&second)?;
    assert_eq!(
        super::prepared::compare(&outputs, &updated)?,
        vec![super::prepared::Difference::Changed(
            outputs[0].target.clone()
        )]
    );
    sqlx::query("UPDATE profile_mods SET enabled=0 WHERE profile_id=?")
        .bind(&profile)
        .execute(&tracker.pool)
        .await?;
    assert_eq!(super::prepared::files(&first)?, outputs);
    let third = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    assert!(super::prepared::files(&third)?.is_empty());
    assert_eq!(
        super::prepared::compare(&outputs, &[])?,
        vec![super::prepared::Difference::Removed(
            outputs[0].target.clone()
        )]
    );
    assert_eq!(
        super::prepared::compare(&[], &outputs)?,
        vec![super::prepared::Difference::Added(
            outputs[0].target.clone()
        )]
    );
    let mut remapped = outputs.clone();
    remapped[0].mod_id = Some("different identity, same bytes".into());
    assert!(super::prepared::compare(&outputs, &remapped)?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn frozen_aurora_outputs_resolve_filename_conflicts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, mut game, profile) = snapshot_fixture(temp.path()).await?;
    game.engine = GameEngine::Aurora;
    for (id, path) in [
        ("loser", "Override/FolderA/file.txt"),
        ("winner", "Override/FolderB/file.txt"),
    ] {
        sqlx::query("UPDATE mod_files SET game_rel_lowercase=?,game_rel_original=? WHERE mod_id=?")
            .bind(path.to_lowercase())
            .bind(path)
            .bind(id)
            .execute(&tracker.pool)
            .await?;
    }
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let frozen = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let outputs = super::prepared::files(&frozen)?;
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].target,
        Target::file(&game.engine, "Override/FolderB/file.txt")?
    );
    game.engine = GameEngine::Bethesda;
    let separate = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    assert_eq!(super::prepared::files(&separate)?.len(), 2);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn damaged_manifests_cannot_reinterpret_sources_or_engine_targets() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let frozen = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut missing = frozen.clone();
    missing
        .sources
        .retain(|source| source.path != "cache/winner");
    assert!(missing.validate().is_err());
    let mut wrong_game = frozen.clone();
    wrong_game.game_id = "another_game".into();
    assert!(wrong_game.validate().is_err());
    let mut wrong_mod = frozen.clone();
    wrong_mod
        .records
        .iter_mut()
        .find(|rows| rows.table == Table::Files)
        .expect("files")
        .rows[0]
        .insert(
            "cache_path".into(),
            serde_json::Value::from("cache/winner/file.txt"),
        );
    assert!(wrong_mod.validate().is_err());
    let mut wrong_engine = frozen.clone();
    wrong_engine.outputs = super::prepared::files(&frozen)?;
    wrong_engine.outputs[0].target = Target::file(&GameEngine::Aurora, "../system/file.txt")?;
    assert!(wrong_engine.validate().is_err());
    let mut incomplete = frozen.clone();
    incomplete
        .records
        .iter_mut()
        .find(|rows| rows.table == Table::ProfileMods)
        .expect("profile mods")
        .rows
        .pop();
    assert!(incomplete.validate().is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn unchanged_managed_files_are_verified_without_replacing_their_hardlinks() -> Result<()> {
    use super::journal::{Journal, Node};
    let temp = tempfile::tempdir()?;
    let (tracker, game, _) = snapshot_fixture(temp.path()).await?;
    fs::create_dir_all(game.data_dir())?;
    let live = game.data_dir().join("file.txt");
    fs::hard_link(temp.path().join("winner/file.txt"), &live)?;
    let inode = fs::metadata(&live)?.ino();
    let mode = fs::metadata(&live)?.permissions().mode() & 0o777;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let identity = history.retain(live.clone(), Control::default()).await?;
    let journal = Journal::prepare(
        &history,
        &game,
        vec![(
            Target::file(&game.engine, "file.txt")?,
            Node::File { identity, mode },
        )],
        Control::default(),
    )
    .await?;
    journal
        .persist(&history, &game, "deploy", Default::default())
        .await?;
    let _applied = journal.apply(&history, &game, Control::default()).await?;
    assert_eq!(fs::metadata(&live)?.ino(), inode);
    fs::write(&live, b"external change")?;
    assert!(
        journal
            .apply(&history, &game, Control::default())
            .await
            .is_err()
    );
    assert!(journal.recover(&history, &game, false).await.is_err());
    assert_eq!(fs::read(live)?, b"external change");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn aliased_engine_targets_cannot_write_the_same_destination_twice() -> Result<()> {
    use super::journal::{Journal, Node};
    let temp = tempfile::tempdir()?;
    let (tracker, game, _) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let targets = vec![
        (Target::file(&game.engine, "file.txt")?, Node::Absent),
        (
            Target::file(&game.engine, "../Data/FILE.txt")?,
            Node::Absent,
        ),
    ];
    assert!(
        Journal::prepare(&history, &game, targets, Control::default())
            .await
            .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn restored_history_preserves_hidden_files_after_the_original_profile_is_deleted()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    fs::write(
        temp.path().join("winner/.deployd-restoration"),
        b"mod-owned hidden content",
    )?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    tx.commit().await?;
    for mod_id in ["winner", "loser", "disabled"] {
        tracker.delete_mod(mod_id).await?;
        fs::remove_dir_all(temp.path().join(mod_id))?;
    }
    sqlx::query("DELETE FROM profiles WHERE id=?")
        .bind(&profile)
        .execute(&tracker.pool)
        .await?;
    let new = super::restore::restore(&history, &id, "Restored", Control::default()).await?;
    let restored = manifest::capture(
        &history,
        &game,
        &new,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let hidden = restored
        .sources
        .iter()
        .find(|source| source.path.ends_with("/.deployd-restoration"))
        .expect("restored hidden file");
    let path = temp
        .path()
        .join(hidden.path.strip_prefix("cache/").expect("cache anchor"));
    assert_eq!(fs::read(path)?, b"mod-owned hidden content");
    assert_eq!(restored.sources.len(), manifest.sources.len());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn unknown_recovery_versions_preserve_the_bound_store_and_journal() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    drop(History::open(&tracker, "game", temp.path(), true).await?);
    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('future','game','deploy',7,'{}')").execute(&tracker.pool).await?;
    assert!(
        History::open(&tracker, "game", temp.path(), true)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        1
    );
    assert!(
        temp.path()
            .join("deployd-history/game/store.json")
            .is_file()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn cancellation_during_historical_verification_creates_no_restored_profile() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (tracker, game, profile) = snapshot_fixture(temp.path()).await?;
    let history = History::open(&tracker, &game.id, temp.path(), true).await?;
    let manifest = manifest::capture(
        &history,
        &game,
        &profile,
        temp.path().to_owned(),
        Control::default(),
    )
    .await?;
    let mut tx = durable(&tracker).await?;
    let id = history.publish(&mut tx, &manifest).await?;
    tx.commit().await?;
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let signal = cancelled.clone();
    let control = Control {
        cancelled,
        progress: Arc::new(move |_, _| signal.store(true, Ordering::Release)),
    };
    assert!(
        super::restore::restore(&history, &id, "Cancelled", control)
            .await
            .is_err()
    );
    assert_eq!(tracker.list_profiles(&game.id).await?.len(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?,
        0
    );
    Ok(())
}

// @variants: both
#[test]
fn generated_objects_deduplicate_with_sources_and_survive_writable_materialization() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let bytes = b"# This file is managed by Deployd\n*Example.esp\n";
    fs::write(&source, bytes)?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    let identity = store.retain_generated(bytes, &control)?;
    assert_eq!(store.retain(&source, &control)?, identity);
    assert_eq!(store.retain_generated(bytes, &control)?, identity);
    let restored = temp.path().join("Plugins.txt");
    store.materialize(&identity, &restored, 0o600, &control)?;
    fs::write(&restored, b"tool changes")?;
    assert_eq!(fs::read(store.source(&identity)?)?, bytes);
    assert_eq!(fs::metadata(store.source(&identity)?)?.nlink(), 1);
    assert_eq!(
        fs::metadata(store.source(&identity)?)?.permissions().mode() & 0o777,
        0o400
    );
    assert_eq!(
        fs::read_dir(temp.path().join("deployd-history/game/objects"))?.count(),
        1
    );
    Ok(())
}

// @variants: both
#[test]
fn cancelled_generated_content_leaves_no_published_or_staged_object() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = Store::create(temp.path(), "game", "store")?;
    let mut control = Control::default();
    let cancelled = control.cancelled.clone();
    control.progress = Arc::new(move |_, _| cancelled.store(true, Ordering::Release));
    assert!(
        store
            .retain_generated(&vec![42; 256 * 1024], &control)
            .is_err()
    );
    assert_eq!(
        fs::read_dir(temp.path().join("deployd-history/game/objects"))?.count(),
        0
    );
    assert_eq!(
        fs::read_dir(temp.path().join("deployd-history/game/staging"))?.count(),
        0
    );
    let empty = store.retain_generated(b"", &Control::default())?;
    assert_eq!(empty.size, 0);
    store.verify(&empty, &Control::default())?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn generated_content_registration_failures_remain_retryable() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let tracker = Tracker::open("sqlite::memory:").await?.tracker;
    let history = History::open(&tracker, "game", temp.path(), true).await?;
    sqlx::query("CREATE TRIGGER reject_generated BEFORE INSERT ON generation_objects BEGIN SELECT RAISE(ABORT,'injected failure'); END")
        .execute(&tracker.pool).await?;
    assert!(
        history
            .retain_generated(b"generated output".to_vec(), Control::default())
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generation_objects")
        .fetch_one(&tracker.pool)
        .await?;
    assert_eq!(count, 0);
    sqlx::query("DROP TRIGGER reject_generated")
        .execute(&tracker.pool)
        .await?;
    let identity = history
        .retain_generated(b"generated output".to_vec(), Control::default())
        .await?;
    history.store.verify(&identity, &Control::default())?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generation_objects")
        .fetch_one(&tracker.pool)
        .await?;
    assert_eq!(count, 1);
    Ok(())
}
