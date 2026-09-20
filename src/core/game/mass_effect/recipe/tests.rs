use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

use tempfile::{TempDir, tempdir};

use super::super::package::PackagePlan;
use super::*;
use crate::models::game::{GameConfig, GameEngine};

const ENGINE: &str = "BioGame/CookedPCConsole/Engine.pcc";
const DLC: &str = "BioGame/DLC/DLC_MOD_Test/CookedPCConsole/Test.pcc";

struct Fixture {
    temp: TempDir,
    data: PathBuf,
    tracker: Tracker,
    game: Game,
    profile: String,
    baseline: Baseline,
}

impl Fixture {
    async fn new(target: Target) -> Result<Self> {
        Self::with_game(target, |_| Ok(())).await
    }

    async fn with_game(
        target: Target,
        initialize: impl FnOnce(&std::path::Path) -> Result<()>,
    ) -> Result<Self> {
        let temp = tempdir()?;
        let data = temp.path().join("data");
        let game = Game {
            id: target.game_id().into(),
            title: "MELE".into(),
            path: temp.path().join("family").join(match target {
                Target::Le1 => "Game/ME1",
                Target::Le2 => "Game/ME2",
                Target::Le3 => "Game/ME3",
            }),
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        fs::create_dir_all(game.path.join("BioGame/CookedPCConsole"))?;
        fs::create_dir_all(game.path.join("Binaries/Win64"))?;
        let exe = match target {
            Target::Le1 => "MassEffect1.exe",
            Target::Le2 => "MassEffect2.exe",
            Target::Le3 => "MassEffect3.exe",
        };
        fs::write(game.path.join("Binaries/Win64").join(exe), b"executable")?;
        fs::write(game.path.join(ENGINE), b"original")?;
        for name in ["Coalesced_INT.bin", "Coalesced_FRA.bin", "PlotManager.pcc"]
            .into_iter()
            .chain(super::super::helper::jobs::BASES.iter().copied())
        {
            if name != "Engine.pcc" {
                fs::write(
                    game.path.join("BioGame/CookedPCConsole").join(name),
                    b"dependency",
                )?;
            }
        }
        initialize(&game.path)?;
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
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("missing baseline")?;
        Ok(Self {
            temp,
            data,
            tracker,
            game,
            profile,
            baseline,
        })
    }

    fn package(&self, content: &[u8], extra: &str) -> Result<(PathBuf, PackagePlan)> {
        let source = self.temp.path().join(Uuid::new_v4().to_string());
        fs::create_dir_all(source.join("DLC_MOD_Test/CookedPCConsole"))?;
        fs::write(
            source.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"),
            content,
        )?;
        let target = match self.game.id.as_str() {
            "mass-effect-le1" => "LE1",
            "mass-effect-le2" => "LE2",
            _ => "LE3",
        };
        fs::write(
            source.join("moddesc.ini"),
            format!(
                "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame={target}\nmodname=Example\nmodver=1.0\nmoddev=Author\nmoddesc=Content\n{extra}\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_Test\ndestdirs=DLC_MOD_Test\n[BASEGAME]\nmoddir=.\nnewfiles=DLC_MOD_Test/CookedPCConsole/Test.pcc\nreplacefiles=BioGame/CookedPCConsole/Engine.pcc\n"
            ),
        )?;
        let plan = PackagePlan::inspect(&source, None)?;
        Ok((source, plan))
    }

    async fn retain(&self, content: &[u8], extra: &str) -> Result<(PathBuf, Package)> {
        let (source, plan) = self.package(content, extra)?;
        let stored = sources::retain_in(
            source.clone(),
            plan,
            None,
            self.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
        Ok((source, stored.selection()))
    }

    fn recipe(&self, packages: Vec<Package>) -> Recipe {
        let target = match self.game.id.as_str() {
            "mass-effect-le1" => Target::Le1,
            "mass-effect-le2" => Target::Le2,
            _ => Target::Le3,
        };
        Recipe {
            components: Vec::new(),
            version: 1,
            backend_version: 1,
            helper_version: None,
            target,
            language: "INT".into(),
            packages,
            launcher: Vec::new(),
        }
    }

    async fn inspect(&self, recipe: Recipe) -> Result<ValidatedRecipe> {
        inspect_in(recipe, self.baseline.clone(), self.data.clone(), cancel()).await
    }

    async fn deploy(
        &self,
        recipe: Recipe,
        previous: Option<&journal::State>,
    ) -> Result<journal::State> {
        let plan = self.inspect(recipe).await?;
        deploy_in(
            self.tracker.clone(),
            Destination {
                repair_components: false,
                backend: None,
                game: self.game.clone(),
                profile: self.profile.clone(),
                previous: previous.map(|state| state.generation.clone()),
            },
            plan,
            self.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await
    }

    fn stored(&self, package: &Package, path: &str) -> PathBuf {
        self.data
            .join("mele-sources")
            .join(&package.source_sha256)
            .join(path)
    }
}

fn cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

// @variants: both
#[tokio::test]
async fn retains_independent_sources_and_rebuilds_after_imports_are_removed() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let fixture = Fixture::new(target).await?;
        let (source, package) = fixture.retain(b"package", "").await?;
        let source_file = source.join("DLC_MOD_Test/CookedPCConsole/Test.pcc");
        let retained = fixture.stored(&package, "DLC_MOD_Test/CookedPCConsole/Test.pcc");
        assert_ne!(
            fs::metadata(&source_file)?.ino(),
            fs::metadata(&retained)?.ino()
        );
        assert_eq!(fs::metadata(&retained)?.nlink(), 1);
        assert_eq!(fs::metadata(&retained)?.permissions().mode() & 0o222, 0);
        fs::remove_dir_all(source)?;
        let recipe = fixture.recipe(vec![package]);
        let installed = fixture.deploy(recipe.clone(), None).await?;
        assert_eq!(installed.recipe, Some(recipe.clone()));
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"package");
        assert_eq!(fs::read(fixture.game.path.join(DLC))?, b"package");
        let rebuilt = fixture.deploy(recipe.clone(), Some(&installed)).await?;
        assert_ne!(rebuilt.generation, installed.generation);
        let mut disabled = recipe.clone();
        disabled.packages[0].enabled = false;
        fixture.deploy(disabled.clone(), Some(&rebuilt)).await?;
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
        assert!(!fixture.game.path.join(DLC).exists());
        assert_eq!(fs::read(retained)?, b"package");
        assert_eq!(
            fixture
                .tracker
                .mele_recipe(&fixture.game.id, &fixture.profile)
                .await?,
            Some(disabled)
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preview_and_deployment_follow_the_same_recipe_order() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, first) = fixture.retain(b"first", "").await?;
    let (_, second) = fixture.retain(b"second", "").await?;
    let recipe = fixture.recipe(vec![first.clone(), second.clone()]);
    let plan = fixture.inspect(recipe.clone()).await?;
    assert_eq!(plan.collisions().len(), 2);
    assert!(
        plan.collisions()
            .iter()
            .all(|collision| collision.replaced == first.id && collision.winner == second.id)
    );
    assert!(plan.files().iter().all(|file| file.package == second.id));
    let installed = fixture.deploy(recipe, None).await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"second");
    fixture
        .deploy(fixture.recipe(vec![second, first]), Some(&installed))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"first");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn dependencies_and_incompatibilities_include_enabled_packages_only() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, missing) = fixture
        .retain(b"missing", "requireddlc=DLC_MOD_Dependency")
        .await?;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![missing]))
            .await
            .is_err()
    );
    let (_, incompatible) = fixture
        .retain(b"conflict", "incompatibledlc=DLC_MOD_Test")
        .await?;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![incompatible.clone()]))
            .await
            .is_err()
    );
    let mut disabled = incompatible;
    disabled.enabled = false;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![disabled]))
            .await?
            .files()
            .is_empty()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn versioned_dependencies_follow_enabled_sources_and_obsolete_dlc_removal() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, _) = fixture.package(b"provider", "")?;
    let manifest = source.join("moddesc.ini");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)?.replace("modver=1.0", "modver=2.0"),
    )?;
    let plan = PackagePlan::inspect(&source, None)?;
    let provider = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection();
    let (source, _) =
        fixture.package(b"dependent", "requireddlc=DLC_MOD_Test[minversion=1.9.0.1]")?;
    let manifest = source.join("moddesc.ini");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)?
            .replace("destdirs=DLC_MOD_Test", "destdirs=DLC_MOD_Dependent"),
    )?;
    let plan = PackagePlan::inspect(&source, None)?;
    let dependent = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection();
    fixture
        .inspect(fixture.recipe(vec![provider.clone(), dependent.clone()]))
        .await?;
    let mut disabled = provider.clone();
    disabled.enabled = false;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![disabled, dependent.clone()]))
            .await
            .is_err()
    );
    let (_, old) = fixture.retain(b"old", "").await?;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![old, dependent.clone()]))
            .await
            .is_err()
    );
    let (source, _) = fixture.package(b"retire", "")?;
    let manifest = source.join("moddesc.ini");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)?.replace(
            "destdirs=DLC_MOD_Test",
            "destdirs=DLC_MOD_Retirement\noutdatedcustomdlc=DLC_MOD_Test",
        ),
    )?;
    let plan = PackagePlan::inspect(&source, None)?;
    let retirement = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection();
    assert!(
        fixture
            .inspect(fixture.recipe(vec![
                provider.clone(),
                retirement.clone(),
                dependent.clone()
            ]))
            .await
            .is_err()
    );
    fixture
        .inspect(fixture.recipe(vec![retirement, provider, dependent]))
        .await?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn altered_sources_and_forged_plans_never_publish_a_retained_package() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, mut plan) = fixture.package(b"payload", "")?;
    let hash = plan.source_sha256.clone();
    plan.files[0].destination = "CookedPCConsole/Forged.pcc".into();
    assert!(
        sources::retain_in(
            source.clone(),
            plan,
            None,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert!(!fixture.data.join("mele-sources").join(hash).exists());
    let plan = PackagePlan::inspect(&source, None)?;
    fs::write(
        source.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"),
        b"changed",
    )?;
    assert!(
        sources::retain_in(
            source,
            plan,
            None,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn damaged_retained_sources_are_preserved_and_block_publication() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, package) = fixture.retain(b"payload", "").await?;
    let recipe = fixture.recipe(vec![package.clone()]);
    let plan = fixture.inspect(recipe.clone()).await?;
    let file = fixture.stored(&package, "DLC_MOD_Test/CookedPCConsole/Test.pcc");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
    fs::write(&file, b"damaged")?;
    fs::set_permissions(&file, fs::Permissions::from_mode(0o400))?;
    assert!(
        deploy_in(
            fixture.tracker.clone(),
            Destination {
                repair_components: false,
                backend: None,
                game: fixture.game.clone(),
                profile: fixture.profile.clone(),
                previous: None
            },
            plan,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(fixture.inspect(recipe).await.is_err());
    let original = PackagePlan::inspect(&source, None)?;
    assert!(
        sources::retain_in(
            source,
            original,
            None,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(file)?, b"damaged");
    Ok(())
}

// @variants: both
#[tokio::test]
async fn failed_recipe_commit_restores_files_and_previous_recipe() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, package) = fixture.retain(b"payload", "").await?;
    let recipe = fixture.recipe(vec![package]);
    let installed = fixture.deploy(recipe.clone(), None).await?;
    sqlx::query("CREATE TRIGGER reject_recipe BEFORE UPDATE ON mele_recipes BEGIN SELECT RAISE(ABORT, 'recipe failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&installed))
            .await
            .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"payload");
    assert_eq!(
        fixture
            .tracker
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?,
        Some(recipe)
    );
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed)
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
async fn cancellation_during_retention_does_not_publish_partial_sources() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, plan) = fixture.package(b"payload", "")?;
    let destination = fixture.data.join("mele-sources").join(&plan.source_sha256);
    let cancelled = cancel();
    let stop = cancelled.clone();
    assert!(
        sources::retain_in(
            source,
            plan,
            None,
            fixture.data.clone(),
            cancelled,
            Arc::new(move |_, _| stop.store(true, Ordering::Release))
        )
        .await
        .is_err()
    );
    assert!(!destination.exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn links_and_writable_retained_sources_are_rejected() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, package) = fixture.retain(b"payload", "").await?;
    let stored = fixture.stored(&package, "DLC_MOD_Test/CookedPCConsole/Test.pcc");
    fs::set_permissions(&stored, fs::Permissions::from_mode(0o600))?;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![package.clone()]))
            .await
            .is_err()
    );
    fs::remove_file(&stored)?;
    symlink(
        source.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"),
        &stored,
    )?;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![package]))
            .await
            .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn portable_recipes_reject_unknown_semantics_and_local_paths() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, package) = fixture.retain(b"payload", "").await?;
    let recipe = fixture.recipe(vec![package]);
    let document = serde_json::to_string(&recipe)?;
    assert!(
        !document.contains(
            fixture
                .temp
                .path()
                .to_str()
                .context("invalid fixture path")?
        )
    );
    assert_eq!(serde_json::from_str::<Recipe>(&document)?, recipe);
    let mut future = recipe.clone();
    future.version = 5;
    assert!(future.validate().is_err());
    future = recipe.clone();
    future.packages[0].source_sha256 = "../escape".into();
    assert!(future.validate().is_err());
    future = recipe.clone();
    future.packages[0].options.insert("unknown-choice".into());
    assert!(fixture.inspect(future).await.is_err());
    let mut json = serde_json::to_value(recipe)?;
    json["baseline_path"] = serde_json::json!("/private/game");
    assert!(serde_json::from_value::<Recipe>(json).is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn transformation_inputs_cannot_be_deployed_as_ordinary_content() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, _) = fixture.package(b"payload", "")?;
    fs::write(
        source.join("DLC_MOD_Test/AutoLoad.ini"),
        "[ME1DLCMOUNT]\nModMount=5",
    )?;
    fs::write(
        source.join("DLC_MOD_Test/CookedPCConsole/ConfigDelta-test.m3cd"),
        "[BioUI.ini Engine.UI]\n+Key=Value",
    )?;
    let plan = PackagePlan::inspect(&source, None)?;
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let recipe = fixture.recipe(vec![stored.selection()]);
    assert!(
        fixture
            .inspect(recipe.clone())
            .await?
            .transformations()
            .contains(&Transformation::ConfigDelta)
    );
    assert!(fixture.deploy(recipe, None).await.is_err());
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(!fixture.game.path.join(DLC).exists());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn le2_alternate_config_delta_updates_its_custom_dlc_ini() -> Result<()> {
    let fixture = Fixture::new(Target::Le2).await?;
    let (source, _) = fixture.package(b"payload", "")?;
    let cooked = source.join("DLC_MOD_Test/CookedPCConsole");
    fs::write(
        cooked.join("BIOGame.ini"),
        "[SFXGame.BioGlobalVariableTable]\r\n+TimedPlotUnlocks=Base\r\n",
    )?;
    let alternate = source.join("Alternates/Choice");
    fs::create_dir_all(&alternate)?;
    fs::write(
        alternate.join("ConfigDelta-choice.m3cd"),
        "[BioGame.ini SFXGame.BioGlobalVariableTable]\n+TimedPlotUnlocks=Selected",
    )?;
    let descriptor = source.join("moddesc.ini");
    let text = fs::read_to_string(&descriptor)?.replace(
        "destdirs=DLC_MOD_Test\n",
        "destdirs=DLC_MOD_Test\naltdlc=((FriendlyName=Choice,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_ADD_FOLDERFILES_TO_CUSTOMDLC,ModAltDLC=Alternates\\Choice,ModDestDLC=DLC_MOD_Test\\CookedPCConsole))\n",
    );
    fs::write(descriptor, text)?;
    let plan = PackagePlan::inspect(&source, None)?;
    let options = plan.option_keys();
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let mut package = stored.selection();
    package.options = options;
    let recipe = fixture.recipe(vec![package]);
    let destination = || Destination {
        repair_components: false,
        backend: None,
        game: fixture.game.clone(),
        profile: fixture.profile.clone(),
        previous: None,
    };
    let plan = inspect_prepared_in(
        fixture.tracker.clone(),
        destination(),
        recipe,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    deploy_in(
        fixture.tracker.clone(),
        destination(),
        plan,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let installed = fs::read_to_string(
        fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Test/CookedPCConsole/BIOGame.ini"),
    )?;
    assert!(installed.contains("+TimedPlotUnlocks=Base"));
    assert!(
        installed.contains("+TimedPlotUnlocks=Selected"),
        "{installed}"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn public_entry_points_honor_cancellation_before_publishing() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, package) = fixture.package(b"payload", "")?;
    let stopped = Arc::new(AtomicBool::new(true));
    assert!(
        sources::retain(source, package, None, stopped.clone(), Arc::new(|_, _| {}))
            .await
            .is_err()
    );
    assert!(
        inspect(
            fixture.recipe(Vec::new()),
            fixture.baseline.clone(),
            stopped.clone()
        )
        .await
        .is_err()
    );
    let plan = fixture.inspect(fixture.recipe(Vec::new())).await?;
    assert!(
        deploy(
            fixture.tracker.clone(),
            Destination {
                repair_components: false,
                backend: None,
                game: fixture.game.clone(),
                profile: fixture.profile.clone(),
                previous: None
            },
            plan,
            stopped,
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn profile_recipes_survive_switching_and_restart() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, package) = fixture.retain(b"payload", "").await?;
    let first = fixture.recipe(vec![package]);
    let installed = fixture.deploy(first.clone(), None).await?;
    let other = fixture
        .tracker
        .create_profile(&fixture.game.id, "Other")
        .await?;
    fixture
        .tracker
        .switch_profile(&fixture.game.id, &other)
        .await?;
    let second = fixture.recipe(Vec::new());
    let plan = fixture.inspect(second.clone()).await?;
    deploy_in(
        fixture.tracker.clone(),
        Destination {
            repair_components: false,
            backend: None,
            game: fixture.game.clone(),
            profile: other.clone(),
            previous: Some(installed.generation),
        },
        plan,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let reopened = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    assert_eq!(
        reopened
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?,
        Some(first.clone())
    );
    assert_eq!(
        reopened.mele_recipe(&fixture.game.id, &other).await?,
        Some(second)
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn abandoning_recipe_deployment_retains_staging_until_rollback_finishes() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, package) = fixture.retain(b"payload", "").await?;
    let plan = fixture.inspect(fixture.recipe(vec![package])).await?;
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
    let task = tokio::spawn(deploy_in(
        fixture.tracker.clone(),
        Destination {
            repair_components: false,
            backend: None,
            game: fixture.game.clone(),
            profile: fixture.profile.clone(),
            previous: None,
        },
        plan,
        fixture.data.clone(),
        cancel(),
        progress,
    ));
    tokio::time::timeout(std::time::Duration::from_secs(15), waiting).await??;
    task.abort();
    let _ = task.await;
    assert!(
        crate::core::location_recovery::activity_lock()
            .try_write_owned()
            .is_err()
    );
    assert!(
        fs::read_dir(fixture.data.join("mele-rebuilds"))?
            .next()
            .is_some()
    );
    release.send(())?;
    let _finished = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        crate::core::location_recovery::activity_lock().write_owned(),
    )
    .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(
        fixture
            .tracker
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?
            .is_none()
    );
    assert!(
        fs::read_dir(fixture.data.join("mele-rebuilds"))?
            .next()
            .is_none()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn enabled_dependencies_can_be_supplied_by_another_recipe_package() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, dependent) = fixture
        .retain(b"dependent", "requireddlc=DLC_MOD_Dependency")
        .await?;
    let (source, _) = fixture.package(b"dependency", "")?;
    fs::rename(
        source.join("DLC_MOD_Test"),
        source.join("DLC_MOD_Dependency"),
    )?;
    let manifest = fs::read_to_string(source.join("moddesc.ini"))?
        .replace("DLC_MOD_Test", "DLC_MOD_Dependency");
    fs::write(source.join("moddesc.ini"), manifest)?;
    let plan = PackagePlan::inspect(&source, None)?;
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let mut dependency = stored.selection();
    assert!(
        fixture
            .inspect(fixture.recipe(vec![dependent.clone(), dependency.clone()]))
            .await
            .is_ok()
    );
    dependency.enabled = false;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![dependent, dependency]))
            .await
            .is_err()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn preview_rejects_colliding_directory_casing_and_future_manifest_versions() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, _) = fixture.package(b"payload", "")?;
    let manifest = fs::read_to_string(source.join("moddesc.ini"))?.replace(
        "BioGame/CookedPCConsole/Engine.pcc",
        "BioGame/cookedpcconsole/Other.pcc",
    );
    fs::write(source.join("moddesc.ini"), manifest)?;
    let plan = PackagePlan::inspect(&source, None)?;
    let stored = sources::retain_in(
        source,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert!(
        fixture
            .inspect(fixture.recipe(vec![stored.selection()]))
            .await
            .is_err()
    );
    let mut selection = stored.selection();
    selection.manifest_version = "99".into();
    assert!(fixture.recipe(vec![selection]).validate().is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_hardlinked_imports_and_preserves_legacy_deployment_state() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (source, _) = fixture.package(b"payload", "")?;
    fs::hard_link(
        source.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"),
        source.join("alias.pcc"),
    )?;
    assert!(PackagePlan::inspect(&source, None).is_err());
    let value = serde_json::json!({ "version": 1, "generation": Uuid::new_v4().to_string(), "profile": fixture.profile, "files": [] });
    let legacy: journal::State = serde_json::from_value(value)?;
    assert!(legacy.recipe.is_none());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn disabling_a_missing_source_restores_originals_without_loading_that_source() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let (_, package) = fixture.retain(b"payload", "").await?;
    let recipe = fixture.recipe(vec![package.clone()]);
    let installed = fixture.deploy(recipe, None).await?;
    fs::remove_dir_all(
        fixture
            .data
            .join("mele-sources")
            .join(&package.source_sha256),
    )?;
    let mut disabled = package;
    disabled.enabled = false;
    fixture
        .deploy(fixture.recipe(vec![disabled]), Some(&installed))
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert!(!fixture.game.path.join(DLC).exists());
    Ok(())
}

mod merges;

mod corpus;

mod installation;

mod runtime;

mod content;
mod playtest;
mod removal;

mod textures;

mod advanced;
mod alternates;
mod preparation;

mod dlc;

mod shaders;

mod binary;
