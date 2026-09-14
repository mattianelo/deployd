use std::fs;

use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::core::game::mass_effect::{family::Family, launcher};
use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameConfig, GameEngine};
use crate::utils::location::{FolderRole, FolderSelection, SelectedLocation};

use super::*;

struct Fixture {
    temp: tempfile::TempDir,
    tracker: Tracker,
    games: Vec<Game>,
    data: PathBuf,
    family: String,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("grant");
        let data = temp.path().join("data");
        fs::create_dir_all(data.join("cache"))?;
        fs::create_dir_all(root.join("Game/Launcher/Content"))?;
        fs::write(
            root.join("Game/Launcher/MassEffectLauncher.exe"),
            b"launcher",
        )?;
        fs::write(root.join("Game/Launcher/bink2w64.dll"), b"original")?;
        let tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("db").display()
        ))
        .await?
        .tracker;
        let mut games = Vec::new();
        for number in 1..=3 {
            let game = Game {
                id: format!("mass-effect-le{number}"),
                title: "MELE".into(),
                path: root.join(format!("Game/ME{number}")),
                data_subdir: "BioGame".into(),
                engine: GameEngine::MassEffect,
                wine_prefix: None,
            };
            fs::create_dir_all(game.path.join("BioGame"))?;
            fs::create_dir_all(game.path.join("Binaries/Win64"))?;
            fs::write(
                game.path
                    .join(format!("Binaries/Win64/MassEffect{number}.exe")),
                b"game",
            )?;
            crate::core::game::mass_effect::baseline::configure(
                &tracker,
                &[GameConfig {
                    game: game.clone(),
                    custom: true,
                    locations: vec![FolderSelection {
                        role: FolderRole::Game,
                        location: SelectedLocation {
                            root: root.clone(),
                            host_hint: None,
                        },
                        relative: format!("Game/ME{number}").into(),
                    }],
                }],
                &[],
                std::sync::Arc::new(|_| {}),
            )
            .await?;
            tracker.ensure_default_profile(&game.id).await?;
            sqlx::query("INSERT INTO settings(key,value) VALUES (?,?)")
                .bind(format!("cache_dir_{}", game.id))
                .bind(data.join("cache").to_str())
                .execute(&tracker.pool)
                .await?;
            games.push(game);
        }
        let location = tracker
            .folder_location(&games[0].id, FolderRole::Game)
            .await?
            .id;
        let hash = format!("{:x}", Sha256::digest(b"original"));
        let family: Family = serde_json::from_value(
            json!({"version":1,"original":{"size":8,"sha256":hash},"owners":[],"installed":false}),
        )?;
        tracker.record_mele_family(location, &family).await?;
        let originals = data
            .join("mele-family-originals")
            .join(location.to_string());
        fs::create_dir_all(&originals)?;
        fs::write(originals.join("bink2w64.dll"), b"original")?;
        Ok(Self {
            temp,
            tracker,
            games,
            data,
            family: location.to_string(),
        })
    }

    fn source(&self, bytes: &[u8]) -> Result<launcher::Entry> {
        let hash = format!("{:x}", Sha256::digest(bytes));
        let tree = format!(
            "{:x}",
            Sha256::digest(format!("Content/Intro.bik\0{}\0{hash}\n", bytes.len()))
        );
        let source = self.data.join("mele-launcher-sources").join(&tree);
        fs::create_dir_all(source.join("Content"))?;
        fs::write(source.join("Content/Intro.bik"), bytes)?;
        Ok(serde_json::from_value(
            json!({"id":uuid::Uuid::new_v4().to_string(),"name":"Intro","enabled":false,"source_sha256":tree,"approval":tree,
            "sources":[{"relative":"Content/Intro.bik","size":bytes.len(),"sha256":hash}],
            "files":[{"source":"Content/Intro.bik","destination":"Content/Intro.bik","identity":{"size":bytes.len(),"sha256":hash}}]}),
        )?)
    }
}

// @variants: both
#[tokio::test]
async fn shared_history_retains_every_game_and_restores_without_original_sources() -> Result<()> {
    let fixture = Fixture::new().await?;
    let game = &fixture.games[0];
    let cache = fixture.data.join("cache");
    let history = History::open(&fixture.tracker, &game.id, &cache, true).await?;
    let first = fixture.source(b"first")?;
    let journal = prepare(
        &history,
        game,
        engine::Action::Mods(vec![first]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    assert!(list(&history, &fixture.family).await?.is_empty());
    apply(&history, game, &journal, Control::default()).await?;
    let entries = list(&history, &fixture.family).await?;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].live);
    assert_eq!(entries[0].references, 0);
    assert!(!entries[0].created_at.is_empty());
    let first_id = entries[0].id.clone();
    let recipe = serde_json::from_value(
        json!({"version":1,"target":"LE1","backend_version":1,"language":"INT","packages":[]}),
    )?;
    let unchanged = prepare(
        &history,
        game,
        engine::Action::Support(recipe),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    apply(&history, game, &unchanged, Control::default()).await?;
    assert_eq!(list(&history, &fixture.family).await?.len(), 1);
    for game in &fixture.games {
        let other = History::bind(
            &fixture.tracker,
            &game.id,
            &cache,
            false,
            history.lease.clone(),
        )
        .await?;
        load(&other, &fixture.family, &first_id, Control::default()).await?;
    }
    assert!(delete(&history, &fixture.family, &first_id).await.is_err());
    let second = fixture.source(b"second")?;
    let journal = prepare(
        &history,
        game,
        engine::Action::Mods(vec![second]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    apply(&history, game, &journal, Control::default()).await?;
    let second_id = list(&history, &fixture.family)
        .await?
        .into_iter()
        .find(|entry| entry.live)
        .context("Missing live revision")?
        .id;
    assert_eq!(
        deletion_size(&history, &fixture.family, &first_id).await?,
        3 * 5
    );
    fs::remove_dir_all(fixture.data.join("mele-launcher-sources"))?;
    fs::remove_dir_all(fixture.data.join("mele-family-originals"))?;
    let journal = restore(
        &history,
        game,
        &fixture.family,
        &first_id,
        Control::default(),
    )
    .await?;
    apply(&history, game, &journal, Control::default()).await?;
    assert_eq!(
        deletion_size(&history, &fixture.family, &second_id).await?,
        3 * 6
    );
    delete(&history, &fixture.family, &second_id).await?;
    assert_eq!(list(&history, &fixture.family).await?.len(), 1);
    assert!(
        !game
            .path
            .parent()
            .context("Missing game parent")?
            .join("Launcher/Content/Intro.bik")
            .exists()
    );
    for game in &fixture.games {
        assert!(fixture.tracker.mele_deployment(&game.id).await?.is_none());
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn shared_recovery_preserves_external_edits_on_both_sides_of_commit() -> Result<()> {
    for committed in [false, true] {
        let fixture = Fixture::new().await?;
        let game = &fixture.games[1];
        let history = History::open(
            &fixture.tracker,
            &game.id,
            &fixture.data.join("cache"),
            true,
        )
        .await?;
        let journal = prepare(
            &history,
            game,
            engine::Action::Mods(vec![fixture.source(b"retained")?]),
            fixture.data.clone(),
            Control::default(),
        )
        .await?;
        journal
            .persist(&history, game, "shared", BTreeMap::new())
            .await?;
        let applied = journal.apply(&history, game, Control::default()).await?;
        if committed {
            applied.commit_shared(&history, game).await?;
        }
        let live = fixture.temp.path().join("grant/Game/Launcher/bink2w64.dll");
        fs::write(&live, b"external")?;
        assert!(journal.recover(&history, game, committed).await.is_err());
        assert_eq!(fs::read(&live)?, b"external");
        assert!(
            prepare(
                &history,
                game,
                engine::Action::Mods(vec![]),
                fixture.data.clone(),
                Control::default()
            )
            .await
            .is_err()
        );
        assert!(
            fixture
                .tracker
                .ensure_no_mele_journal(&fixture.games[2].id)
                .await
                .is_err()
        );
        fs::write(&live, b"original")?;
        crate::core::generations::recovery::recover_journal(&history, game).await?;
        assert_eq!(
            list(&history, &fixture.family).await?.len(),
            usize::from(committed)
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Uses the pinned launcher proxy cache; all games and deployment files are disposable"]
async fn shared_payload_activation_restores_files_and_recovers_failed_commit() -> Result<()> {
    let fixture = Fixture::new().await?;
    engine::seed_test_proxy(&fixture.data).await?;
    let game = &fixture.games[2];
    let history = History::open(
        &fixture.tracker,
        &game.id,
        &fixture.data.join("cache"),
        true,
    )
    .await?;
    let mut first = fixture.source(b"first")?;
    first.enabled = true;
    let journal = prepare(
        &history,
        game,
        engine::Action::Mods(vec![first]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    let live = fixture
        .temp
        .path()
        .join("grant/Game/Launcher/Content/Intro.bik");
    assert!(!live.exists());
    apply(&history, game, &journal, Control::default()).await?;
    assert_eq!(fs::read(&live)?, b"first");
    let id = list(&history, &fixture.family).await?[0].id.clone();
    let mut second = fixture.source(b"second")?;
    second.enabled = true;
    let journal = prepare(
        &history,
        game,
        engine::Action::Mods(vec![second]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    sqlx::query("CREATE TRIGGER fail_shared_commit BEFORE UPDATE ON mele_families BEGIN SELECT RAISE(ABORT,'injected shared commit failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(
        apply(&history, game, &journal, Control::default())
            .await
            .is_err()
    );
    assert_eq!(fs::read(&live)?, b"first");
    assert_eq!(list(&history, &fixture.family).await?.len(), 1);
    sqlx::query("DROP TRIGGER fail_shared_commit")
        .execute(&fixture.tracker.pool)
        .await?;
    let cleared = prepare(
        &history,
        game,
        engine::Action::Mods(vec![]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    apply(&history, game, &cleared, Control::default()).await?;
    assert!(!live.exists());
    fs::remove_dir_all(fixture.data.join("mele-launcher-sources"))?;
    fs::remove_dir_all(fixture.data.join("mele-components"))?;
    fs::remove_dir_all(fixture.data.join("mele-family-originals"))?;
    let restoring = restore(&history, game, &fixture.family, &id, Control::default()).await?;
    apply(&history, game, &restoring, Control::default()).await?;
    assert_eq!(fs::read(live)?, b"first");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn game_generations_protect_shared_dependencies_and_block_mismatched_restoration()
-> Result<()> {
    use crate::core::game::mass_effect::recipe::Destination;
    use crate::core::generations::{coordinator, manifest, mele, state};
    use crate::core::save_manager::SaveSetId;

    let fixture = Fixture::new().await?;
    let game = &fixture.games[0];
    let history = History::open(
        &fixture.tracker,
        &game.id,
        &fixture.data.join("cache"),
        true,
    )
    .await?;
    let shared = prepare(
        &history,
        game,
        engine::Action::Mods(vec![fixture.source(b"first")?]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    apply(&history, game, &shared, Control::default()).await?;
    let first = list(&history, &fixture.family).await?[0].id.clone();
    let profile = fixture.tracker.ensure_default_profile(&game.id).await?.id;
    let mut manifest = manifest::capture(
        &history,
        game,
        &profile,
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    let recipe = serde_json::from_value(
        json!({"version":1,"target":"LE1","backend_version":1,"language":"INT","packages":[]}),
    )?;
    let journal = mele::prepare(
        &history,
        Destination {
            game: game.clone(),
            profile: profile.clone(),
            previous: None,
            repair_components: false,
            backend: None,
        },
        &mut manifest,
        recipe,
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    assert_eq!(
        manifest.shared_revision,
        Some((fixture.family.clone(), first.clone()))
    );
    sqlx::query(
        "INSERT INTO generation_game_state(game_id,live_save_mode,modified) VALUES (?,'global',0)",
    )
    .bind(&game.id)
    .execute(&fixture.tracker.pool)
    .await?;
    let mut tx = durable(&fixture.tracker).await?;
    let previous = state::read(&mut tx, &game.id).await?;
    tx.rollback().await?;
    coordinator::activate(
        &history,
        game,
        &journal,
        previous.as_ref(),
        Some(&state::Deployment {
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
    let unknown = fixture
        .temp
        .path()
        .join("grant/Game/Launcher/ASI/unmanaged.asi");
    fs::create_dir_all(unknown.parent().context("Missing ASI parent")?)?;
    fs::write(&unknown, b"unmanaged")?;
    assert!(
        verify_dependency(
            &history,
            game,
            &journal
                .mele
                .as_ref()
                .context("Missing MELE participant")?
                .desired,
            journal.dependency.as_ref()
        )
        .await
        .is_err()
    );
    fs::remove_file(unknown)?;
    let generation = manifest.id()?;
    history.load(&generation).await?;
    assert!(dependency_matches(&history, game, &manifest).await?);
    let shared = prepare(
        &history,
        game,
        engine::Action::Mods(vec![fixture.source(b"second")?]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    apply(&history, game, &shared, Control::default()).await?;
    assert!(!dependency_matches(&history, game, &manifest).await?);
    assert!(delete(&history, &fixture.family, &first).await.is_err());
    let restored = crate::core::generations::restore::restore(
        &history,
        &generation,
        "Restored",
        Control::default(),
    )
    .await?;
    let mut draft = manifest::capture(
        &history,
        game,
        &restored,
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    assert!(
        mele::reuse(
            &history,
            game,
            &mut draft,
            fixture.data.clone(),
            Control::default()
        )
        .await
        .is_err()
    );
    let restoring = restore(&history, game, &fixture.family, &first, Control::default()).await?;
    apply(&history, game, &restoring, Control::default()).await?;
    assert!(
        mele::reuse(
            &history,
            game,
            &mut draft,
            fixture.data.clone(),
            Control::default()
        )
        .await?
        .is_some()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn shared_recovery_uses_reauthorized_bindings_and_rejects_unrelated_roots() -> Result<()> {
    let fixture = Fixture::new().await?;
    let mut game = fixture.games[0].clone();
    let history = History::open(
        &fixture.tracker,
        &game.id,
        &fixture.data.join("cache"),
        true,
    )
    .await?;
    let journal = prepare(
        &history,
        &game,
        engine::Action::Mods(vec![fixture.source(b"payload")?]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    journal
        .persist(&history, &game, "shared", BTreeMap::new())
        .await?;
    let old = fixture.temp.path().join("grant");
    let new = fixture.temp.path().join("renewed-grant");
    fs::rename(&old, &new)?;
    assert!(journal.recover(&history, &game, false).await.is_err());
    game.path = new.join("Game/ME1");
    assert!(journal.recover(&history, &game, false).await.is_err());
    sqlx::query("UPDATE folder_locations SET root=? WHERE id=?")
        .bind(new.to_str())
        .bind(fixture.family.parse::<i64>()?)
        .execute(&fixture.tracker.pool)
        .await?;
    journal.recover(&history, &game, false).await?;
    assert_eq!(
        fs::read(new.join("Game/Launcher/bink2w64.dll"))?,
        b"original"
    );
    assert!(list(&history, &fixture.family).await?.is_empty());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn shared_apply_blocks_missing_affected_caches_and_corrupt_retained_objects() -> Result<()> {
    let fixture = Fixture::new().await?;
    let game = &fixture.games[0];
    let history = History::open(
        &fixture.tracker,
        &game.id,
        &fixture.data.join("cache"),
        true,
    )
    .await?;
    let external = fixture.temp.path().join("external-cache");
    fs::create_dir(&external)?;
    sqlx::query("UPDATE settings SET value=? WHERE key=?")
        .bind(external.to_str())
        .bind(format!("cache_dir_{}", fixture.games[2].id))
        .execute(&fixture.tracker.pool)
        .await?;
    let source = fixture.source(b"payload")?;
    let record = serde_json::to_value(&source)?;
    let source_root = fixture.data.join("mele-launcher-sources").join(
        record["source_sha256"]
            .as_str()
            .context("Missing source identity")?,
    );
    fs::write(source_root.join("unrecorded.txt"), b"extra")?;
    assert!(
        prepare(
            &history,
            game,
            engine::Action::Mods(vec![source]),
            fixture.data.clone(),
            Control::default()
        )
        .await
        .is_err()
    );
    fs::remove_file(source_root.join("unrecorded.txt"))?;
    let journal = prepare(
        &history,
        game,
        engine::Action::Mods(vec![fixture.source(b"payload")?]),
        fixture.data.clone(),
        Control::default(),
    )
    .await?;
    let offline = fixture.temp.path().join("offline");
    fs::rename(&external, &offline)?;
    assert!(
        apply(&history, game, &journal, Control::default())
            .await
            .is_err()
    );
    assert!(list(&history, &fixture.family).await?.is_empty());
    fs::rename(&offline, &external)?;
    let other = History::bind(
        &fixture.tracker,
        &fixture.games[2].id,
        &external,
        false,
        history.lease.clone(),
    )
    .await?;
    let object = journal
        .shared
        .as_ref()
        .context("Missing shared intent")?
        .revision
        .payloads()?
        .into_values()
        .find(|id| id.size == 7)
        .context("Missing retained source")?;
    let path = other.store.source(&object)?;
    fs::remove_file(&path)?;
    fs::write(&path, b"corrupt")?;
    assert!(
        apply(&history, game, &journal, Control::default())
            .await
            .is_err()
    );
    assert!(list(&history, &fixture.family).await?.is_empty());
    fs::remove_file(&path)?;
    let repaired = history.store.source(&object)?;
    fs::copy(repaired, &path)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
    other.store.verify(&object, &Control::default())?;
    apply(&history, game, &journal, Control::default()).await?;
    Ok(())
}
