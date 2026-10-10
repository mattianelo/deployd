use std::fs;
use std::os::unix::fs::PermissionsExt;

use anyhow::{Context, Result};

use super::{GameLoadMode, ensure_vanilla_snapshot, load_game_data};
use crate::core::generations::{activation, api, content::Control};
use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};

struct Fixture {
    tracker: Tracker,
    pool: sqlx::SqlitePool,
    game: Game,
    profile: String,
    generation: String,
    root: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Result<Self> {
        Self::for_engine(GameEngine::Aurora).await
    }

    async fn for_engine(engine: GameEngine) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let database = format!(
            "sqlite://{}?mode=rwc",
            root.path().join("startup.db").display()
        );
        let tracker = Tracker::open(&database).await?.tracker;
        let pool = sqlx::SqlitePool::connect(&database).await?;
        let game = Game {
            id: if engine == GameEngine::Bethesda {
                "skyrim-se"
            } else {
                "startup"
            }
            .into(),
            title: "Startup".into(),
            path: root.path().join("game"),
            data_subdir: "Data".into(),
            wine_prefix: (engine == GameEngine::Bethesda).then(|| root.path().join("prefix")),
            engine,
        };
        fs::create_dir_all(game.data_dir())?;
        if let Some(prefix) = &game.wine_prefix {
            fs::create_dir_all(prefix.join("drive_c/users/player"))?;
        }
        tracker
            .upsert_game(
                &game.id,
                &game.title,
                &game.path,
                &game.data_subdir,
                game.engine.as_str(),
                game.wine_prefix.as_deref(),
                true,
            )
            .await?;
        let profile = tracker.ensure_default_profile(&game.id).await?.id;
        let source = root.path().join("mod/file.bin");
        fs::create_dir_all(source.parent().context("Source parent")?)?;
        fs::write(&source, vec![7u8; 1024 * 1024])?;
        sqlx::query(
            "INSERT INTO mods(id,game_id,name,enabled,priority) VALUES ('mod',?,'Mod',1,1)",
        )
        .bind(&game.id)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO mod_files(mod_id,game_rel_lowercase,game_rel_original,cache_path) VALUES ('mod','file.bin','file.bin',?)").bind(source.to_str()).execute(&pool).await?;
        tracker.save_to_profile(&profile, &game.id).await?;
        let generation = activation::prepare(
            &tracker,
            &game,
            root.path(),
            Some(&profile),
            false,
            Control::default(),
        )
        .await?
        .activate(Control::default())
        .await?
        .context("Generation")?;
        Ok(Self {
            tracker,
            pool,
            game,
            profile,
            generation,
            root,
        })
    }

    async fn open(&self) -> Result<super::LoadedData> {
        load_game_data(&self.tracker, &self.game, GameLoadMode::OpenGame)
            .await
            .map_err(anyhow::Error::msg)
    }
}

// @variants: both
#[tokio::test]
async fn opening_a_game_defers_bulk_hashing_but_integrity_checks_detect_live_edits() -> Result<()> {
    let fixture = Fixture::new().await?;
    fs::write(fixture.game.data_dir().join("file.bin"), b"external edit")?;
    crate::utils::verified_files::invalidate()?;
    let before = crate::utils::verified_files::metrics().hashed_bytes;
    let loaded = fixture.open().await?;
    assert!(loaded.location_accessible);
    assert_eq!(loaded.mods.len(), 1);
    assert_eq!(crate::utils::verified_files::metrics().hashed_bytes, before);
    let report = api::verify_integrity(
        &fixture.tracker,
        &fixture.game,
        fixture.root.path(),
        Control::default(),
    )
    .await?;
    assert_eq!(report.differing_files, 1);
    assert!(report.hashed_bytes >= 1024 * 1024);
    fixture.open().await?;
    let modified: bool =
        sqlx::query_scalar("SELECT modified FROM generation_game_state WHERE game_id=?")
            .bind(&fixture.game.id)
            .fetch_one(&fixture.pool)
            .await?;
    assert!(modified);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn opening_defers_retained_content_checks_but_deploy_and_restore_reject_damage() -> Result<()>
{
    let fixture = Fixture::new().await?;
    let hash: String =
        sqlx::query_scalar("SELECT sha256 FROM generation_objects WHERE game_id=? LIMIT 1")
            .bind(&fixture.game.id)
            .fetch_one(&fixture.pool)
            .await?;
    let retained = crate::utils::paths::generation_store_in(fixture.root.path(), &fixture.game.id)?
        .join("objects")
        .join(hash)
        .join("content");
    fs::set_permissions(&retained, fs::Permissions::from_mode(0o600))?;
    fs::write(&retained, b"damaged retained content")?;
    assert!(fixture.open().await?.location_accessible);
    assert!(
        api::verify_integrity(
            &fixture.tracker,
            &fixture.game,
            fixture.root.path(),
            Control::default()
        )
        .await
        .is_err()
    );
    assert!(
        api::restore(
            &fixture.tracker,
            &fixture.game,
            fixture.root.path(),
            &fixture.generation,
            "Restored",
            Control::default()
        )
        .await
        .is_err()
    );
    assert!(
        activation::prepare(
            &fixture.tracker,
            &fixture.game,
            fixture.root.path(),
            Some(&fixture.profile),
            false,
            Control::default()
        )
        .await
        .is_err()
    );
    assert_eq!(
        fs::read(fixture.game.data_dir().join("file.bin"))?,
        vec![7u8; 1024 * 1024]
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn opening_keeps_failed_recovery_blocked_and_preserves_its_journal() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES ('pending',?,'deploy',1,'{}')")
        .bind(&fixture.game.id).execute(&fixture.pool).await?;
    let loaded = fixture.open().await?;
    assert!(!loaded.location_accessible);
    assert!(
        loaded
            .access_warnings
            .iter()
            .any(|warning| warning.contains("recovery"))
    );
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM generation_journals WHERE game_id=?")
            .bind(&fixture.game.id)
            .fetch_one(&fixture.pool)
            .await?;
    assert_eq!(pending, 1);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn reopening_preserves_the_original_vanilla_snapshot() -> Result<()> {
    let fixture = Fixture::new().await?;
    ensure_vanilla_snapshot(&fixture.tracker, &fixture.game)
        .await
        .map_err(anyhow::Error::msg)?;
    let before = fixture
        .tracker
        .get_vanilla_metadata(&fixture.game.id)
        .await?;
    assert!(!before.is_empty());
    fs::write(fixture.game.data_dir().join("new.bin"), b"new")?;
    ensure_vanilla_snapshot(&fixture.tracker, &fixture.game)
        .await
        .map_err(anyhow::Error::msg)?;
    assert_eq!(
        fixture
            .tracker
            .get_vanilla_metadata(&fixture.game.id)
            .await?,
        before
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn bethesda_opening_skips_bulk_content_and_blocks_missing_folder_access() -> Result<()> {
    let fixture = Fixture::for_engine(GameEngine::Bethesda).await?;
    crate::utils::verified_files::invalidate()?;
    let plugin_bytes = crate::core::game::plugins_txt_paths(&fixture.game)
        .iter()
        .map(|path| fs::metadata(path).map(|metadata| metadata.len()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .sum::<u64>();
    let before = crate::utils::verified_files::metrics().hashed_bytes;
    assert!(fixture.open().await?.location_accessible);
    assert_eq!(
        crate::utils::verified_files::metrics().hashed_bytes - before,
        plugin_bytes
    );
    fs::rename(&fixture.game.path, fixture.root.path().join("unavailable"))?;
    let loaded = fixture.open().await?;
    assert!(!loaded.location_accessible);
    assert!(!loaded.access_warnings.is_empty());
    assert_eq!(loaded.mods.len(), 1);
    Ok(())
}
