use std::fs;

use super::*;

struct Fixture {
    history: History,
    game: Game,
    profile: String,
    _temp: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let (tracker, mut game, profile) =
            super::super::tests::snapshot_fixture(temp.path()).await?;
        game.id = "skyrim-se".into();
        game.wine_prefix = Some(temp.path().join("prefix"));
        fs::create_dir_all(temp.path().join("prefix/drive_c/users/player"))?;
        sqlx::query("UPDATE profiles SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        sqlx::query("UPDATE mods SET game_id=?")
            .bind(&game.id)
            .execute(&tracker.pool)
            .await?;
        sqlx::query(
            "UPDATE mod_files SET game_rel_lowercase='test.esp',game_rel_original='Test.esp'",
        )
        .execute(&tracker.pool)
        .await?;
        for (id, name, order) in [
            ("loser", "TEST.esp", 5),
            ("winner", "Test.esp", 7),
            ("disabled", "test.ESP", 1),
        ] {
            sqlx::query(
                "INSERT INTO plugins(id,mod_id,filename,load_order,enabled) VALUES (?,?,?,?,1)",
            )
            .bind(id)
            .bind(id)
            .bind(name)
            .bind(order)
            .execute(&tracker.pool)
            .await?;
            sqlx::query("INSERT INTO profile_plugins(profile_id,plugin_id,load_order,enabled) VALUES (?,?,?,1)")
                .bind(&profile).bind(id).bind(order).execute(&tracker.pool).await?;
        }
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        Ok(Self {
            history,
            game,
            profile,
            _temp: temp,
        })
    }

    async fn capture(&self) -> Result<Manifest> {
        super::super::manifest::capture(
            &self.history,
            &self.game,
            &self.profile,
            self.history.cache.clone(),
            Control::default(),
        )
        .await
    }
}

// @variants: both
#[tokio::test]
async fn frozen_plugin_configuration_follows_the_deployed_file_winner() -> Result<()> {
    let fixture = Fixture::new().await?;
    let manifest = fixture.capture().await?;
    let bytes = render(&manifest)?;
    assert_eq!(bytes, b"# This file is managed by Deployd\n*Test.esp\n");
    sqlx::query("UPDATE profile_plugins SET enabled=0 WHERE plugin_id='winner'")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let updated = fixture.capture().await?;
    assert_eq!(render(&manifest)?, bytes);
    assert_eq!(
        render(&updated)?,
        b"# This file is managed by Deployd\nTest.esp\n"
    );
    sqlx::query("UPDATE profile_mods SET enabled=0 WHERE mod_id='winner'")
        .execute(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(
        render(&fixture.capture().await?)?,
        b"# This file is managed by Deployd\n*TEST.esp\n"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn disabled_plugins_remain_recorded_in_the_frozen_load_order() -> Result<()> {
    let fixture = Fixture::new().await?;
    sqlx::query("UPDATE plugins SET filename='Disabled.esp' WHERE id='disabled'")
        .execute(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(
        render(&fixture.capture().await?)?,
        b"# This file is managed by Deployd\nDisabled.esp\n*Test.esp\n"
    );
    sqlx::query("UPDATE profile_mods SET enabled=0")
        .execute(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(
        render(&fixture.capture().await?)?,
        b"# This file is managed by Deployd\nDisabled.esp\nTest.esp\n"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn generated_plugin_outputs_are_retained_without_writing_live_configuration() -> Result<()> {
    let fixture = Fixture::new().await?;
    let manifest = fixture.capture().await?;
    sqlx::query("UPDATE plugins SET enabled=0,load_order=99")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let outputs = plugins(
        &fixture.history,
        &fixture.game,
        &manifest,
        Control::default(),
    )
    .await?;
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].content, outputs[1].content);
    for (slot, output) in outputs.iter().enumerate() {
        assert_eq!(output.target, Target::PluginControl { slot });
        assert_eq!(output.mod_id, None);
        assert!(!output.target.resolve(&fixture.game)?.exists());
        let identity = output.content.as_ref().context("Generated content")?;
        assert_eq!(
            fs::read(fixture.history.store.source(identity)?)?,
            b"# This file is managed by Deployd\n*Test.esp\n"
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn missing_winner_metadata_and_unsafe_plugin_names_block_preparation() -> Result<()> {
    let fixture = Fixture::new().await?;
    for filename in [
        "Good.esp\n*Injected.esp",
        "../Escape.esp",
        "#Comment.esp",
        "*AlreadyEnabled.esp",
    ] {
        sqlx::query("UPDATE plugins SET filename=? WHERE id='winner'")
            .bind(filename)
            .execute(&fixture.history.tracker.pool)
            .await?;
        assert!(render(&fixture.capture().await?).is_err());
    }
    sqlx::query("DELETE FROM profile_plugins WHERE plugin_id='winner'")
        .execute(&fixture.history.tracker.pool)
        .await?;
    sqlx::query("DELETE FROM plugins WHERE id='winner'")
        .execute(&fixture.history.tracker.pool)
        .await?;
    assert!(render(&fixture.capture().await?).is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn plugin_preparation_rejects_other_engines_and_lost_prefix_access() -> Result<()> {
    let fixture = Fixture::new().await?;
    let manifest = fixture.capture().await?;
    for engine in [
        GameEngine::Aurora,
        GameEngine::Eclipse,
        GameEngine::REDEngine,
        GameEngine::MassEffect,
    ] {
        let mut other = manifest.clone();
        other.engine = engine;
        assert!(render(&other).is_err());
    }
    for prefix in [None, Some(fixture.history.cache.join("missing-prefix"))] {
        let mut game = fixture.game.clone();
        game.wine_prefix = prefix;
        assert!(
            plugins(&fixture.history, &game, &manifest, Control::default())
                .await
                .is_err()
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM generations")
        .fetch_one(&fixture.history.tracker.pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn activates_plugin_files_and_frozen_order_from_the_same_prepared_generation() -> Result<()> {
    use super::super::journal::{Journal, Node};
    use super::super::state::{Deployment, State};
    use crate::core::save_manager::SaveSetId;
    use crate::models::manifest::ModFile;

    let fixture = Fixture::new().await?;
    fs::create_dir_all(fixture.game.data_dir())?;
    let mut manifest = fixture.capture().await?;
    manifest.outputs = super::super::prepared::files(&manifest)?;
    manifest.outputs.extend(
        plugins(
            &fixture.history,
            &fixture.game,
            &manifest,
            Control::default(),
        )
        .await?,
    );
    for path in game::plugins_txt_paths(&fixture.game) {
        fs::create_dir_all(path.parent().context("Configuration parent")?)?;
        fs::write(path, b"Old.esp\n")?;
    }
    let desired = manifest
        .outputs
        .iter()
        .map(|output| {
            Ok((
                output.target.clone(),
                Node::File {
                    identity: output.content.clone().context("Output content")?,
                    mode: output.mode,
                },
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let journal =
        Journal::prepare(&fixture.history, &fixture.game, desired, Control::default()).await?;
    let previous = State {
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
    sqlx::query("UPDATE profile_plugins SET enabled=0,load_order=99")
        .execute(&fixture.history.tracker.pool)
        .await?;
    let files = vec![ModFile {
        mod_id: "winner".into(),
        game_rel_lowercase: "test.esp".into(),
        game_rel_original: "Test.esp".into(),
        cache_path: fixture
            .history
            .cache
            .join("winner/file.txt")
            .to_string_lossy()
            .into_owned(),
    }];
    super::super::coordinator::activate(
        &fixture.history,
        &fixture.game,
        &journal,
        Some(&previous),
        Some(&Deployment {
            manifest: &manifest,
            profile: &fixture.profile,
            files: &files,
        }),
        &previous.saves,
        Control::default(),
    )
    .await?;
    assert_eq!(
        fs::read(fixture.game.data_dir().join("Test.esp"))?,
        b"winner"
    );
    for path in game::plugins_txt_paths(&fixture.game) {
        assert_eq!(
            fs::read(path)?,
            b"# This file is managed by Deployd\n*Test.esp\n"
        );
    }
    assert_eq!(fixture.history.load(&manifest.id()?).await?, manifest);
    Ok(())
}
