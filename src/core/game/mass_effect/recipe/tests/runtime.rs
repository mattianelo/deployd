use super::*;
use crate::core::game::mass_effect::components;

async fn deploy_runtime(
    fixture: &Fixture,
    recipe: Recipe,
    previous: Option<&journal::State>,
    repair: bool,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<journal::State> {
    let plan = fixture.inspect(recipe).await?;
    deploy_in(
        fixture.tracker.clone(),
        Destination {
            backend: None,
            game: fixture.game.clone(),
            profile: fixture.profile.clone(),
            previous: previous.map(|state| state.generation.clone()),
            repair_components: repair,
        },
        plan,
        fixture.data.clone(),
        cancelled,
        progress,
    )
    .await
}

// @variants: both
#[tokio::test]
#[ignore = "Downloads pinned runtime components from upstream; operates only on disposable fixtures"]
async fn installs_repairs_and_removes_real_runtime_components_transactionally() -> Result<()> {
    let fixture = Fixture::with_game(Target::Le1, |game| {
        fs::write(game.join(components::BINK), b"game-owned original")?;
        Ok(())
    })
    .await?;
    let mut recipe = fixture.recipe(Vec::new());
    recipe.enable_runtime_components();
    components::test_cache::seed(&fixture.data, &recipe.components).await?;
    let plan = fixture.inspect(recipe.clone()).await?;
    assert_eq!(plan.component_files().len(), 7);
    let installed = deploy_runtime(
        &fixture,
        recipe.clone(),
        None,
        false,
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(installed.version, 2);
    assert_eq!(installed.recipe, Some(recipe.clone()));
    assert_eq!(
        fs::read(fixture.game.path.join(components::ORIGINAL))?,
        b"game-owned original"
    );
    let rebuilt = deploy_runtime(
        &fixture,
        recipe.clone(),
        Some(&installed),
        false,
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(rebuilt.files, installed.files);
    fs::remove_file(fixture.game.path.join(components::BINK))?;
    fs::remove_file(
        fixture
            .game
            .path
            .join("Binaries/Win64/ASI/AutoTOCLE-v2.asi"),
    )?;
    assert!(
        deploy_runtime(
            &fixture,
            recipe.clone(),
            Some(&rebuilt),
            false,
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    let cancelled = cancel();
    let signal = cancelled.clone();
    assert!(
        deploy_runtime(
            &fixture,
            recipe.clone(),
            Some(&rebuilt),
            true,
            cancelled,
            Arc::new(move |done, total| {
                if total == 6000 && done > 1000 {
                    signal.store(true, Ordering::Release);
                }
            })
        )
        .await
        .is_err()
    );
    assert!(!fixture.game.path.join(components::BINK).exists());
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    let repaired = deploy_runtime(
        &fixture,
        recipe.clone(),
        Some(&rebuilt),
        true,
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(repaired.files, installed.files);
    let bink = fixture.game.path.join(components::BINK);
    let proxy = fs::read(&bink)?;
    fs::write(&bink, b"external edit")?;
    assert!(
        deploy_runtime(
            &fixture,
            fixture.recipe(Vec::new()),
            Some(&repaired),
            false,
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(&bink)?, b"external edit");
    fs::write(&bink, &proxy)?;
    sqlx::query("CREATE TRIGGER reject_runtime BEFORE UPDATE ON mele_recipes BEGIN SELECT RAISE(ABORT, 'runtime failure'); END").execute(&fixture.tracker.pool).await?;
    assert!(
        deploy_runtime(
            &fixture,
            fixture.recipe(Vec::new()),
            Some(&repaired),
            false,
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(&bink)?, proxy);
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(repaired.clone())
    );
    assert_eq!(
        fixture
            .tracker
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?,
        Some(recipe.clone())
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    sqlx::query("DROP TRIGGER reject_runtime")
        .execute(&fixture.tracker.pool)
        .await?;
    fs::remove_file(
        fixture
            .game
            .path
            .join("Binaries/Win64/ASI/AutoTOCLE-v2.asi"),
    )?;
    let removed = deploy_runtime(
        &fixture,
        fixture.recipe(Vec::new()),
        Some(&repaired),
        true,
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert!(removed.files.is_empty());
    assert_eq!(fs::read(&bink)?, b"game-owned original");
    for file in installed
        .files
        .iter()
        .filter(|file| file.relative != components::BINK)
    {
        assert!(
            !fixture.game.path.join(&file.relative).exists(),
            "{}",
            file.relative
        );
    }
    for target in [Target::Le2, Target::Le3] {
        let other = Fixture::with_game(target, |game| {
            fs::write(game.join(components::BINK), b"other original")?;
            Ok(())
        })
        .await?;
        let mut recipe = other.recipe(Vec::new());
        recipe.enable_runtime_components();
        components::test_cache::seed(&other.data, &recipe.components).await?;
        let installed =
            deploy_runtime(&other, recipe, None, false, cancel(), Arc::new(|_, _| {})).await?;
        assert_eq!(installed.files.len(), 3);
        deploy_runtime(
            &other,
            other.recipe(Vec::new()),
            Some(&installed),
            false,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
        assert_eq!(
            fs::read(other.game.path.join(components::BINK))?,
            b"other original"
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn runtime_recipes_export_versions_without_local_component_claims() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let mut recipe = fixture.recipe(Vec::new());
    recipe.enable_runtime_components();
    recipe.validate()?;
    let json = serde_json::to_value(&recipe)?;
    for component in json["components"]
        .as_array()
        .context("missing components")?
    {
        assert_eq!(component.as_object().context("invalid component")?.len(), 2);
        assert!(component.get("component").is_some());
        assert!(component.get("version").is_some());
    }
    assert_eq!(serde_json::from_value::<Recipe>(json.clone())?, recipe);
    let mut claims = json;
    claims["components"][0]["installed_path"] = serde_json::json!("/private/game");
    assert!(serde_json::from_value::<Recipe>(claims).is_err());
    recipe.version = 1;
    assert!(recipe.validate().is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Downloads the pinned proxy and runtimes; uses only disposable shared-family fixtures"]
async fn shared_launcher_survives_other_game_restoration_and_rolls_back_failures() -> Result<()> {
    let fixture = Fixture::with_game(Target::Le1, |root| {
        fs::write(root.join(components::BINK), b"game original")?;
        Ok(())
    })
    .await?;
    let family_root = fixture.temp.path().join("family");
    let launcher = family_root.join("Game/Launcher");
    let original = fs::read(launcher.join("bink2w64.dll"))?;
    let mut recipe = fixture.recipe(Vec::new());
    recipe.enable_runtime_components();
    components::test_cache::seed(&fixture.data, &recipe.components).await?;
    let location = fixture
        .tracker
        .folder_location(&fixture.game.id, crate::utils::location::FolderRole::Game)
        .await?;
    let cancelled = cancel();
    let signal = cancelled.clone();
    assert!(
        deploy_runtime(
            &fixture,
            recipe.clone(),
            None,
            false,
            cancelled,
            Arc::new(move |done, total| {
                if total == 6000 && done > 1000 && done < 6000 {
                    signal.store(true, Ordering::Release);
                }
            })
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, original);
    assert!(!launcher.join("bink2w64_original.dll").exists());
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_none()
    );
    let cancelled = cancel();
    let signal = cancelled.clone();
    let interrupted_launcher = launcher.clone();
    assert!(
        deploy_runtime(
            &fixture,
            recipe.clone(),
            None,
            false,
            cancelled,
            Arc::new(move |done, total| {
                if total == 6000 && done > 1000 && done < 6000 {
                    let _ = fs::write(interrupted_launcher.join("bink2w64.dll"), b"outside edit");
                    signal.store(true, Ordering::Release);
                }
            })
        )
        .await
        .is_err()
    );
    assert!(
        fixture
            .tracker
            .mele_journal(&fixture.game.id)
            .await?
            .is_some()
    );
    assert!(
        fixture
            .tracker
            .ensure_no_mele_journal(Target::Le2.game_id())
            .await
            .is_err()
    );
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, b"outside edit");
    let identity = components::proxy_identity();
    let cached = fixture
        .data
        .join("mele-components/artifacts")
        .join(identity.sha256);
    fs::write(launcher.join("bink2w64.dll"), fs::read(cached)?)?;
    let reopened = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    journal::recover_in(reopened, fixture.game.clone(), fixture.data.clone()).await?;
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, original);
    assert!(!launcher.join("bink2w64_original.dll").exists());
    let installed = deploy_runtime(
        &fixture,
        recipe.clone(),
        None,
        false,
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let proxy = fs::read(launcher.join("bink2w64.dll"))?;
    assert_ne!(proxy, original);
    assert_eq!(fs::read(launcher.join("bink2w64_original.dll"))?, original);
    let mut other = fixture.game.clone();
    other.id = Target::Le2.game_id().into();
    other.path = family_root.join("Game/ME2");
    fs::create_dir_all(other.path.join("Binaries/Win64"))?;
    fs::create_dir_all(other.path.join("BioGame"))?;
    fs::write(other.path.join("Binaries/Win64/MassEffect2.exe"), b"exe")?;
    fs::write(other.path.join(components::BINK), b"second game original")?;
    super::super::super::baseline::configure(
        &fixture.tracker,
        &[GameConfig {
            game: other.clone(),
            custom: true,
            locations: vec![crate::utils::location::FolderSelection {
                role: crate::utils::location::FolderRole::Game,
                location: location.selection.clone(),
                relative: "Game/ME2".into(),
            }],
        }],
        &[],
        Arc::new(|_| {}),
    )
    .await?;
    let profile = fixture.tracker.ensure_default_profile(&other.id).await?.id;
    let mut other_recipe = recipe.clone();
    other_recipe.target = Target::Le2;
    other_recipe.enable_runtime_components();
    let baseline = fixture
        .tracker
        .load_mele_baseline(&other.id)
        .await?
        .context("missing baseline")?;
    let plan = inspect_in(
        other_recipe,
        baseline.clone(),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    let second = deploy_in(
        fixture.tracker.clone(),
        Destination {
            repair_components: false,
            backend: None,
            game: other.clone(),
            profile: profile.clone(),
            previous: None,
        },
        plan,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    deploy_runtime(
        &fixture,
        fixture.recipe(Vec::new()),
        Some(&installed),
        false,
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, proxy);
    fs::write(launcher.join("bink2w64.dll"), b"external edit")?;
    let mut empty = fixture.recipe(Vec::new());
    empty.target = Target::Le2;
    let destination = || Destination {
        repair_components: true,
        backend: None,
        game: other.clone(),
        profile: profile.clone(),
        previous: Some(second.generation.clone()),
    };
    let plan = inspect_in(
        empty.clone(),
        baseline.clone(),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    assert!(
        deploy_in(
            fixture.tracker.clone(),
            destination(),
            plan,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, b"external edit");
    fs::write(launcher.join("bink2w64.dll"), &proxy)?;
    sqlx::query("CREATE TRIGGER reject_launcher_commit BEFORE UPDATE ON mele_families BEGIN SELECT RAISE(ABORT, 'family failure'); END").execute(&fixture.tracker.pool).await?;
    let family_before = fixture.tracker.mele_family(location.id).await?;
    let plan = inspect_in(
        empty.clone(),
        baseline.clone(),
        fixture.data.clone(),
        cancel(),
    )
    .await?;
    assert!(
        deploy_in(
            fixture.tracker.clone(),
            destination(),
            plan,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, proxy);
    assert_eq!(
        fixture.tracker.mele_family(location.id).await?,
        family_before
    );
    assert_eq!(
        fixture.tracker.mele_deployment(&other.id).await?,
        Some(second.clone())
    );
    sqlx::query("DROP TRIGGER reject_launcher_commit")
        .execute(&fixture.tracker.pool)
        .await?;
    fs::remove_file(launcher.join("bink2w64.dll"))?;
    let plan = inspect_in(empty, baseline, fixture.data.clone(), cancel()).await?;
    deploy_in(
        fixture.tracker.clone(),
        destination(),
        plan,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(fs::read(launcher.join("bink2w64.dll"))?, original);
    assert!(!launcher.join("bink2w64_original.dll").exists());
    Ok(())
}
