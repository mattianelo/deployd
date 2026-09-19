use std::fs;

use super::*;
use crate::models::game::GameConfig;

struct Fixture {
    _temp: tempfile::TempDir,
    data: PathBuf,
    source: PathBuf,
    tracker: Tracker,
    game: Game,
    profile: String,
}

impl Fixture {
    async fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let data = temp.path().join("data");
        let source = temp.path().join("archive");
        fs::create_dir_all(source.join("CookedPCConsole"))?;
        fs::write(source.join("CookedPCConsole/Engine.pcc"), b"mod content")?;
        let game = Game {
            id: Target::Le1.game_id().into(),
            title: "LE1".into(),
            path: temp.path().join("family").join(match Target::Le1 {
                Target::Le1 => "Game/ME1",
                Target::Le2 => "Game/ME2",
                Target::Le3 => "Game/ME3",
            }),
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        fs::create_dir_all(game.path.join("Binaries/Win64"))?;
        fs::create_dir_all(game.path.join("BioGame/CookedPCConsole"))?;
        fs::write(game.path.join("Binaries/Win64/MassEffect1.exe"), b"exe")?;
        fs::write(
            game.path.join("Binaries/Win64/bink2w64.dll"),
            b"original library",
        )?;
        fs::write(
            game.path.join("BioGame/CookedPCConsole/Engine.pcc"),
            b"original content",
        )?;
        let launcher = temp.path().join("family/Game/Launcher");
        fs::create_dir_all(&launcher)?;
        fs::write(launcher.join("MassEffectLauncher.exe"), b"launcher")?;
        fs::write(launcher.join("bink2w64.dll"), b"original launcher library")?;
        let tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("tracker.db").display()
        ))
        .await?
        .tracker;
        super::super::baseline::configure(
            &tracker,
            &[GameConfig {
                game: game.clone(),
                custom: true,
                locations: vec![crate::utils::location::FolderSelection {
                    role: crate::utils::location::FolderRole::Game,
                    location: crate::utils::location::SelectedLocation {
                        root: temp.path().join("family"),
                        host_hint: None,
                    },
                    relative: game.path.strip_prefix(temp.path().join("family"))?.into(),
                }],
            }],
            &[],
            std::sync::Arc::new(|_| {}),
        )
        .await?;
        let profile = tracker.ensure_default_profile(&game.id).await?.id;
        Ok(Self {
            _temp: temp,
            data,
            source,
            tracker,
            game,
            profile,
        })
    }
    fn request(&self, replace: Option<String>) -> Result<Import> {
        Ok(Import {
            binary_approved: false,
            source: self.source.clone(),
            plan: PackagePlan::inspect(&self.source, Some(Target::Le1))?,
            game: self.game.clone(),
            name: "Example".into(),
            options: BTreeSet::new(),
            nexus: None,
            archive_hash: None,
            archive_path: None,
            replace,
        })
    }
    async fn import(&self, replace: Option<String>) -> Result<AddResult> {
        import_in(
            self.tracker.clone(),
            self.request(replace)?,
            self.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await
    }
}
fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

// @variants: both
#[tokio::test]
async fn combined_restore_prepares_and_applies_in_one_operation() -> Result<()> {
    use super::super::application;

    let fixture = Fixture::new().await?;
    application::deploy_in(
        fixture.tracker.clone(),
        application::Request {
            game: fixture.game.clone(),
            profile: fixture.profile.clone(),
            language: "INT".into(),
            purge: true,
            repair: false,
        },
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let state = fixture
        .tracker
        .mele_deployment(&fixture.game.id)
        .await?
        .context("missing combined deployment state")?;
    assert_eq!(state.profile, fixture.profile);
    assert!(
        state
            .recipe
            .context("missing combined recipe")?
            .packages
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn alternate_only_import_tracks_selected_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    fs::remove_dir_all(fixture.source.join("CookedPCConsole"))?;
    fs::create_dir_all(fixture.source.join("Optional/CookedPCConsole"))?;
    fs::write(
        fixture.source.join("Optional/CookedPCConsole/Content.pcc"),
        b"alternate content",
    )?;
    fs::write(
        fixture.source.join("moddesc.ini"),
        "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame=LE1\nmodname=Alternate only\nmodver=1\nmoddev=Deployd\nmoddesc=Fixture\n[CUSTOMDLC]\naltdlc=((FriendlyName=Install,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_ADD_CUSTOMDLC,ModAltDLC=Optional,ModDestDLC=DLC_MOD_Optional))\n",
    )?;
    let mut request = fixture.request(None)?;
    request.options = request.plan.default_options();
    let imported = import_in(
        fixture.tracker.clone(),
        request,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let expected = "DLC/DLC_MOD_Optional/CookedPCConsole/Content.pcc";
    let tracked = fixture
        .tracker
        .get_mod_files(&imported.mod_entry.id)
        .await?;
    assert_eq!(tracked.len(), 1);
    assert_eq!(tracked[0].game_rel_original, expected);

    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "requires the maintainer-supplied reported Sheploo directory"]
async fn transform_only_import_tracks_merged_package_outputs() -> Result<()> {
    let source = fs::canonicalize("modTesting/Sheploo Appearance Consistency Project")?;
    let plan = PackagePlan::inspect(&source, Some(Target::Le2))?;
    let data = tempfile::tempdir()?;
    let stored = recipe::sources::retain_in(
        source,
        plan,
        None,
        data.path().to_path_buf(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let package = stored.selection();
    let files = tracked_files("sheploo", &stored, &package.options)?;
    assert!(files.iter().any(|file| {
        file.game_rel_original == "CookedPCConsole/SFXGame.pcc"
            && file.cache_path.ends_with("MergeMods/SHEPLOO.m3m")
    }));
    Ok(())
}

// @variants: both
#[tokio::test]
async fn removal_only_mods_retain_sources_and_preview_their_effect() -> Result<()> {
    use super::super::application;
    let fixture = Fixture::new().await?;
    fs::remove_dir_all(fixture.source.join("CookedPCConsole"))?;
    fs::write(
        fixture.source.join("moddesc.ini"),
        "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame=LE1\nmodname=Replacement\nmodver=1.0\nmoddev=Author\nmoddesc=Retires DLC\n[CUSTOMDLC]\noutdatedcustomdlc=DLC_OLD\n",
    )?;
    let imported = fixture.import(None).await?;
    assert_eq!(imported.files_cached, 0);
    let record = fixture
        .tracker
        .mele_package(&imported.mod_entry.id)
        .await?
        .context("missing package")?;
    assert!(
        fixture
            .data
            .join("mele-sources")
            .join(record.package.source_sha256)
            .join("moddesc.ini")
            .is_file()
    );
    let request = || application::Request {
        game: fixture.game.clone(),
        profile: fixture.profile.clone(),
        language: "INT".into(),
        purge: false,
        repair: false,
    };
    application::preview_in(
        fixture.tracker.clone(),
        request(),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    let unexpected = fixture.game.path.join("BioGame/DLC/DLC_OLD/user.txt");
    fs::create_dir_all(unexpected.parent().context("missing parent")?)?;
    fs::write(&unexpected, b"external")?;
    assert!(
        application::preview_in(
            fixture.tracker.clone(),
            request(),
            fixture.data.clone(),
            cancel()
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(unexpected)?, b"external");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn library_import_replacement_and_profiles_retain_package_identity() -> Result<()> {
    let fixture = Fixture::new().await?;
    let installed = fixture.import(None).await?;
    let id = installed.mod_entry.id;
    let first = fixture
        .tracker
        .mele_package(&id)
        .await?
        .context("missing package")?;
    fixture.tracker.toggle_mod(&id, false).await?;
    fixture
        .tracker
        .update_priorities(&[(id.clone(), 42)])
        .await?;
    fixture
        .tracker
        .save_to_profile(&fixture.profile, &fixture.game.id)
        .await?;
    fs::write(
        fixture.source.join("CookedPCConsole/Engine.pcc"),
        b"new mod content",
    )?;
    let updated = fixture.import(Some(id.clone())).await?;
    assert_eq!(updated.mod_entry.id, id);
    assert!(!updated.mod_entry.enabled);
    assert_eq!(updated.mod_entry.priority, 42);
    let second = fixture
        .tracker
        .mele_package(&id)
        .await?
        .context("missing update")?;
    assert_ne!(first.package.source_sha256, second.package.source_sha256);
    assert_eq!(
        fs::read(fixture.game.path.join("BioGame/CookedPCConsole/Engine.pcc"))?,
        b"original content"
    );
    fixture
        .tracker
        .switch_profile(&fixture.game.id, &fixture.profile)
        .await?;
    let recipe = desired(&fixture.tracker, &fixture.game, "INT".into(), false).await?;
    assert_eq!(recipe.packages.len(), 1);
    assert!(!recipe.packages[0].enabled);
    assert!(recipe.components.is_empty());
    fixture.tracker.delete_mod(&id).await?;
    assert!(fixture.tracker.mele_package(&id).await?.is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn library_registration_failure_and_changed_staging_publish_no_mod() -> Result<()> {
    let fixture = Fixture::new().await?;
    let request = fixture.request(None)?;
    fs::write(
        fixture.source.join("CookedPCConsole/Engine.pcc"),
        b"changed",
    )?;
    assert!(
        import_in(
            fixture.tracker.clone(),
            request,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .is_empty()
    );
    sqlx::query("CREATE TRIGGER reject_mele_package BEFORE INSERT ON mele_packages BEGIN SELECT RAISE(ABORT, 'injected failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(fixture.import(None).await.is_err());
    assert!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .is_empty()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_files")
        .fetch_one(&fixture.tracker.pool)
        .await?;
    assert_eq!(count, 0);
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Downloads pinned runtime components; only disposable game fixtures are changed"]
async fn application_import_preview_deploy_disable_and_purge_use_the_same_recipe() -> Result<()> {
    use super::super::{application, components};
    let fixture = Fixture::new().await?;
    components::test_cache::seed(&fixture.data, &components::required(Target::Le1)).await?;
    let installed = fixture.import(None).await?;
    let request = |purge| application::Request {
        game: fixture.game.clone(),
        profile: fixture.profile.clone(),
        language: "INT".into(),
        purge,
        repair: false,
    };
    let preview = application::preview_in(
        fixture.tracker.clone(),
        request(false),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    fixture
        .tracker
        .toggle_mod(&installed.mod_entry.id, false)
        .await?;
    assert!(
        application::apply_in(
            fixture.tracker.clone(),
            preview,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    fixture
        .tracker
        .toggle_mod(&installed.mod_entry.id, true)
        .await?;
    application::deploy_in(
        fixture.tracker.clone(),
        request(false),
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(
        fs::read(fixture.game.path.join("BioGame/CookedPCConsole/Engine.pcc"))?,
        b"mod content"
    );
    let recipe = fixture
        .tracker
        .mele_recipe(&fixture.game.id, &fixture.profile)
        .await?
        .context("missing deployed recipe")?;
    assert_eq!(recipe.packages[0].id, installed.mod_entry.id);
    assert_eq!(recipe.components.len(), 4);
    fixture
        .tracker
        .toggle_mod(&installed.mod_entry.id, false)
        .await?;
    let preview = application::preview_in(
        fixture.tracker.clone(),
        request(false),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    application::apply_in(
        fixture.tracker.clone(),
        preview,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(
        fs::read(fixture.game.path.join("BioGame/CookedPCConsole/Engine.pcc"))?,
        b"original content"
    );
    assert_eq!(
        fs::read(fixture.game.path.join("Binaries/Win64/bink2w64.dll"))?,
        b"original library"
    );
    let preview = application::preview_in(
        fixture.tracker.clone(),
        request(true),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    application::apply_in(
        fixture.tracker.clone(),
        preview,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(fixture.tracker.list_mods(&fixture.game.id).await?.len(), 1);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn library_removal_is_atomic_and_preserves_sources_and_game_files() -> Result<()> {
    let fixture = Fixture::new().await?;
    let id = fixture.import(None).await?.mod_entry.id;
    fixture
        .tracker
        .save_to_profile(&fixture.profile, &fixture.game.id)
        .await?;
    assert!(
        remove(
            &fixture.tracker,
            &fixture.game,
            &[id.clone(), "missing".into()]
        )
        .await
        .is_err()
    );
    assert!(fixture.tracker.mele_package(&id).await?.is_some());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_files WHERE mod_id = ?")
        .bind(&id)
        .fetch_one(&fixture.tracker.pool)
        .await?;
    assert_eq!(count, 1);
    let stored = fixture
        .tracker
        .mele_package(&id)
        .await?
        .context("missing record")?;
    remove(&fixture.tracker, &fixture.game, std::slice::from_ref(&id)).await?;
    assert!(
        fixture
            .tracker
            .list_mods(&fixture.game.id)
            .await?
            .is_empty()
    );
    assert!(fixture.tracker.mele_package(&id).await?.is_none());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM profile_mods WHERE mod_id = ?")
        .bind(&id)
        .fetch_one(&fixture.tracker.pool)
        .await?;
    assert_eq!(count, 0);
    assert!(
        fixture
            .data
            .join("mele-sources")
            .join(stored.package.source_sha256)
            .exists()
    );
    assert_eq!(
        fs::read(fixture.game.path.join("BioGame/CookedPCConsole/Engine.pcc"))?,
        b"original content"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn profile_clone_preserves_mele_language_without_claiming_a_deployment() -> Result<()> {
    let fixture = Fixture::new().await?;
    fixture.import(None).await?;
    let recipe = desired(&fixture.tracker, &fixture.game, "DE".into(), false).await?;
    sqlx::query("INSERT INTO mele_recipes (game_id, profile_id, document) VALUES (?, ?, ?)")
        .bind(&fixture.game.id)
        .bind(&fixture.profile)
        .bind(serde_json::to_string(&recipe)?)
        .execute(&fixture.tracker.pool)
        .await?;
    let cloned = fixture
        .tracker
        .clone_profile(&fixture.profile, "German", &fixture.game.id)
        .await?;
    assert_eq!(
        fixture
            .tracker
            .mele_recipe(&fixture.game.id, &cloned)
            .await?,
        Some(recipe)
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
async fn preview_checks_resolved_texture_runtime_ownership() -> Result<()> {
    use super::super::application;
    let fixture = Fixture::new().await?;
    fs::create_dir_all(fixture.source.join("DLC_MOD_Texture/CookedPCConsole"))?;
    fs::write(
        fixture
            .source
            .join("DLC_MOD_Texture/CookedPCConsole/Content.pcc"),
        b"content",
    )?;
    fs::write(
        fixture.source.join("moddesc.ini"),
        "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame=LE1\nmodname=Texture runtime dependency\nmodver=1\nmoddev=Deployd\nmoddesc=Fixture\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_Texture\ndestdirs=DLC_MOD_Texture\n[ASIMODS]\nasimodstoinstall=((GroupID=88))\n",
    )?;
    fixture.import(None).await?;
    let request = || application::Request {
        game: fixture.game.clone(),
        profile: fixture.profile.clone(),
        language: "INT".into(),
        purge: false,
        repair: false,
    };
    application::preview_in(
        fixture.tracker.clone(),
        request(),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    let unmanaged = fixture
        .game
        .path
        .join("Binaries/Win64/ASI/LE1TextureOverride-v3.asi");
    fs::create_dir_all(unmanaged.parent().context("missing parent")?)?;
    fs::write(&unmanaged, b"unmanaged runtime")?;
    assert!(
        application::preview_in(
            fixture.tracker.clone(),
            request(),
            fixture.data.clone(),
            cancel()
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(unmanaged)?, b"unmanaged runtime");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn binary_consent_survives_restart_and_requires_reapproval_for_changed_sources() -> Result<()>
{
    use super::super::binary;
    let fixture = Fixture::new().await?;
    fs::write(fixture.source.join("Test.asi"), binary::tests::plugin())?;
    assert!(fixture.import(None).await.is_err());
    assert!(!fixture.data.join("mele-sources").exists());
    let mut request = fixture.request(None)?;
    request.binary_approved = true;
    let imported = import_in(
        fixture.tracker.clone(),
        request,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let id = imported.mod_entry.id;
    let tracker = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture._temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    let record = tracker
        .mele_package(&id)
        .await?
        .context("Missing approved package")?;
    assert_eq!(record.version, 2);
    assert!(
        record
            .package
            .binary_approval
            .as_ref()
            .is_some_and(|approval| approval.matches(&fixture.request(None).unwrap().plan))
    );
    fixture.import(Some(id.clone())).await?;
    assert!(fixture.import(None).await.is_err());
    fs::write(
        fixture.source.join("CookedPCConsole/Engine.pcc"),
        b"updated content",
    )?;
    assert!(fixture.import(Some(id.clone())).await.is_err());
    assert_eq!(
        tracker
            .mele_package(&id)
            .await?
            .context("Missing package")?
            .package
            .source_sha256,
        record.package.source_sha256
    );
    let mut request = fixture.request(Some(id.clone()))?;
    request.binary_approved = true;
    import_in(
        tracker.clone(),
        request,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_ne!(
        tracker
            .mele_package(&id)
            .await?
            .context("Missing package")?
            .package
            .source_sha256,
        record.package.source_sha256
    );
    Ok(())
}
