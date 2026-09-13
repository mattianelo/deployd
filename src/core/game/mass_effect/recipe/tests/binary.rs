use super::*;
use crate::core::game::mass_effect::{binary, components};

const DLL: &str = "Binaries/Win64/Example.dll";
const ASI: &str = "Binaries/Win64/ASI/Example.asi";

async fn retain_binary(fixture: &Fixture, target: Target, bytes: &[u8]) -> Result<Package> {
    let source = fixture.temp.path().join(Uuid::new_v4().to_string());
    fs::create_dir_all(source.join("Binaries/Win64/ASI"))?;
    fs::create_dir_all(source.join("CookedPCConsole"))?;
    fs::write(source.join(DLL), bytes)?;
    fs::write(source.join(ASI), bytes)?;
    fs::write(source.join("CookedPCConsole/Engine.pcc"), b"content")?;
    let plan = PackagePlan::inspect(&source, Some(target))?;
    let approval = binary::Approval::for_plan(&plan);
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
    package.binary_approval = Some(approval);
    Ok(package)
}

// @variants: both
#[tokio::test]
async fn binary_recipe_resolves_runtime_dependencies_and_checks_consent() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let fixture = Fixture::with_game(target, |game| {
            fs::write(game.join(components::BINK), b"original bink")?;
            Ok(())
        })
        .await?;
        let package = retain_binary(&fixture, target, &binary::tests::plugin()).await?;
        let mut recipe = fixture.recipe(vec![package]);
        recipe.version = 3;
        let plan = fixture.inspect(recipe.clone()).await?;
        assert!(
            plan.recipe
                .components
                .iter()
                .any(|selection| selection.component == components::Component::BinkProxy)
        );
        assert!(
            plan.recipe
                .components
                .iter()
                .any(|selection| selection.component == components::Component::VisualCpp)
        );
        assert_eq!(plan.recipe.packages[0].binary_files.len(), 2);
        assert!(
            plan.files
                .iter()
                .any(|file| file.destination.relative == DLL)
        );
        recipe.packages[0].binary_approval = None;
        assert!(fixture.inspect(recipe).await.is_err());
    }
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Uses pinned runtime artifacts from the persistent test cache; disposable games only"]
async fn binary_mods_rebuild_repair_and_restore_originals_for_all_three_games() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let fixture = Fixture::with_game(target, |game| {
            fs::write(game.join(components::BINK), b"original bink")?;
            fs::write(game.join(DLL), b"original dll")?;
            Ok(())
        })
        .await?;
        let first = binary::tests::plugin();
        let mut second = first.clone();
        second[256] = 1;
        let a = retain_binary(&fixture, target, &first).await?;
        let b = retain_binary(&fixture, target, &second).await?;
        let mut recipe = fixture.recipe(vec![a, b]);
        recipe.version = 3;
        let plan = fixture.inspect(recipe.clone()).await?;
        assert_eq!(plan.collisions.len(), 3);
        components::test_cache::seed(&fixture.data, &plan.recipe.components).await?;
        let installed = fixture.deploy(recipe.clone(), None).await?;
        assert_eq!(installed.version, 4);
        assert_eq!(fs::read(fixture.game.path.join(DLL))?, second);
        assert_eq!(fs::read(fixture.game.path.join(ASI))?, second);
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"content");
        let reopened = fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .context("Missing deployment")?;
        assert_eq!(reopened, installed);
        let mut persisted = reopened.recipe.clone().context("Missing recipe")?;
        persisted.packages[1].enabled = false;
        let rebuilt = fixture.deploy(persisted.clone(), Some(&reopened)).await?;
        assert_eq!(fs::read(fixture.game.path.join(DLL))?, first);
        assert_eq!(fs::read(fixture.game.path.join(ASI))?, first);
        fs::remove_file(fixture.game.path.join(ASI))?;
        assert!(
            fixture
                .deploy(persisted.clone(), Some(&rebuilt))
                .await
                .is_err()
        );
        let plan = fixture.inspect(persisted).await?;
        let repaired = deploy_in(
            fixture.tracker.clone(),
            Destination {
                repair_components: true,
                backend: None,
                game: fixture.game.clone(),
                profile: fixture.profile.clone(),
                previous: Some(rebuilt.generation.clone()),
            },
            plan,
            fixture.data.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
        assert_eq!(fs::read(fixture.game.path.join(ASI))?, first);
        fs::write(fixture.game.path.join(DLL), b"external edit")?;
        assert!(
            fixture
                .deploy(fixture.recipe(Vec::new()), Some(&repaired))
                .await
                .is_err()
        );
        assert_eq!(fs::read(fixture.game.path.join(DLL))?, b"external edit");
        fs::write(fixture.game.path.join(DLL), &first)?;
        sqlx::query("CREATE TRIGGER reject_binary BEFORE UPDATE ON mele_recipes BEGIN SELECT RAISE(ABORT, 'failure'); END").execute(&fixture.tracker.pool).await?;
        assert!(
            fixture
                .deploy(fixture.recipe(Vec::new()), Some(&repaired))
                .await
                .is_err()
        );
        assert_eq!(fs::read(fixture.game.path.join(DLL))?, first);
        assert_eq!(
            fixture.tracker.mele_deployment(&fixture.game.id).await?,
            Some(repaired.clone())
        );
        sqlx::query("DROP TRIGGER reject_binary")
            .execute(&fixture.tracker.pool)
            .await?;
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&repaired))
            .await?;
        assert_eq!(fs::read(fixture.game.path.join(DLL))?, b"original dll");
        assert!(!fixture.game.path.join(ASI).exists());
        assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
        assert_eq!(fs::read(fixture.stored(&recipe.packages[0], DLL))?, first);
        assert_eq!(fs::read(fixture.stored(&recipe.packages[1], DLL))?, second);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn root_companions_survive_content_transformations_and_failed_rebuilds() -> Result<()> {
    let fixture = Fixture::new(Target::Le1).await?;
    let source = fixture.temp.path().join("companion");
    let relative = "Binaries/Win64/Example.ini";
    fs::create_dir_all(source.join("Binaries/Win64"))?;
    fs::write(source.join(relative), b"settings")?;
    let plan = PackagePlan::inspect(&source, Some(Target::Le1))?;
    let approval = binary::Approval::for_plan(&plan);
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
    package.binary_approval = Some(approval);
    let merge = fixture
        .merge_package("DLC_MOD_Merge", 10, &["config"])
        .await?;
    let mut recipe = fixture.merge_recipe(vec![package, merge]);
    recipe.version = 3;
    let installed = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            fixture.backend("")?,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(fixture.game.path.join(relative))?, b"settings");
    assert!(
        fixture
            .deploy_merges(
                recipe,
                Some(&installed),
                fixture.backend("raise RuntimeError('failed transformation')")?,
                cancel(),
                Arc::new(|_, _| {})
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(fixture.game.path.join(relative))?, b"settings");
    assert_eq!(
        fixture.tracker.mele_deployment(&fixture.game.id).await?,
        Some(installed.clone())
    );
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&installed))
        .await?;
    assert!(!fixture.game.path.join(relative).exists());
    Ok(())
}
