use super::super::super::m3gs;
use super::super::merges::identity;
use super::*;
use helper::Job;

async fn package(fixture: &Fixture, dlc: &str, mount: i32, name: &str) -> Result<Package> {
    let game = fixture.recipe(Vec::new()).target;
    let root = fixture.temp.path().join(Uuid::new_v4().to_string());
    let cooked = root.join(dlc).join("CookedPCConsole");
    fs::create_dir_all(&cooked)?;
    fs::write(
        cooked.join(format!("GlobalShader-0-{name}.m3gs")),
        m3gs::tests::dxbc(mount as u8),
    )?;
    if game == Target::Le1 {
        fs::write(
            root.join(dlc).join("AutoLoad.ini"),
            format!("[ME1DLCMOUNT]\nModMount={mount}"),
        )?;
    } else {
        let mut bytes = vec![0; 108];
        let header: &[u32] = if game == Target::Le2 {
            &[684, 168, 65643]
        } else {
            &[1, 685, 205, 196715]
        };
        for (index, value) in header.iter().enumerate() {
            bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        let offset = if game == Target::Le2 { 12 } else { 16 };
        bytes[offset..offset + 4].copy_from_slice(&mount.to_le_bytes());
        fs::write(cooked.join("Mount.dlc"), bytes)?;
    }
    let game_name = serde_json::to_value(game)?;
    fs::write(
        root.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={}\nmodname=Shader example\nmodver=1.0\nmoddev=Author\nmoddesc=Example\n[CUSTOMDLC]\nsourcedirs={dlc}\ndestdirs={dlc}\n",
            game_name.as_str().context("Missing game")?
        ),
    )?;
    let plan = PackagePlan::inspect(&root, Some(game))?;
    assert!(
        plan.required_transformations()
            .contains(&Transformation::GlobalShader)
    );
    Ok(sources::retain_in(
        root,
        plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection())
}

async fn lifecycle(fixture: &Fixture, backend: helper::Backend) -> Result<()> {
    let high = package(fixture, "DLC_MOD_High", 20, "high").await?;
    let low = package(fixture, "DLC_MOD_Low", 5, "low").await?;
    let mut recipe = fixture.merge_recipe(vec![high, low]);
    let plan = fixture.inspect(recipe.clone()).await?;
    assert!(plan.generated_files().contains(m3gs::TARGET));
    let mut current = plan.merges.originals.clone();
    for file in &plan.files {
        let input = identity(&file.destination)?;
        current.insert(input.path.clone(), input);
    }
    let Some(Job::Shaders {
        game,
        contributions,
        ..
    }) = plan.merges.job(3, &current)?
    else {
        anyhow::bail!("Missing shader job");
    };
    assert_eq!(game, recipe.target);
    assert_eq!(
        contributions
            .iter()
            .map(|item| item.mount)
            .collect::<Vec<_>>(),
        [5, 20]
    );
    let path = fixture.game.path.join("BioGame").join(m3gs::TARGET);
    let original = fs::read(&path)?;
    let first = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    let merged = fs::read(&path)?;
    assert_ne!(merged, original);
    let rebuilt = fixture
        .deploy_merges(
            recipe.clone(),
            Some(&first),
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(fs::read(&path)?, merged);
    recipe.packages[0].enabled = false;
    let disabled = fixture
        .deploy_merges(
            recipe,
            Some(&rebuilt),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    fixture
        .deploy(fixture.recipe(Vec::new()), Some(&disabled))
        .await?;
    assert_eq!(fs::read(&path)?, original);
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_High/CookedPCConsole/GlobalShader-0-high.m3gs")
            .exists()
    );
    assert!(
        !fixture
            .game
            .path
            .join("BioGame/DLC/DLC_MOD_Low/CookedPCConsole/GlobalShader-0-low.m3gs")
            .exists()
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn shader_recipes_rebuild_and_restore_each_game_through_the_journal() -> Result<()> {
    for game in [Target::Le1, Target::Le2, Target::Le3] {
        let fixture = Fixture::with_game(game, |root| {
            fs::write(
                root.join("BioGame").join(m3gs::TARGET),
                b"baseline shader cache",
            )?;
            Ok(())
        })
        .await?;
        lifecycle(&fixture, fixture.backend("")?).await?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn shader_recipes_reject_same_dlc_indices_and_preserve_failed_deployments() -> Result<()> {
    let fixture = Fixture::with_game(Target::Le1, |root| {
        fs::write(
            root.join("BioGame").join(m3gs::TARGET),
            b"baseline shader cache",
        )?;
        Ok(())
    })
    .await?;
    let first = package(&fixture, "DLC_MOD_A", 5, "first").await?;
    let duplicate = package(&fixture, "DLC_MOD_A", 5, "duplicate").await?;
    assert!(
        fixture
            .inspect(fixture.merge_recipe(vec![first.clone(), duplicate]))
            .await
            .is_err()
    );
    let recipe = fixture.merge_recipe(vec![first]);
    let initial = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            fixture.backend("")?,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    let path = fixture.game.path.join("BioGame").join(m3gs::TARGET);
    let previous = fs::read(&path)?;
    assert!(
        fixture
            .deploy_merges(
                recipe,
                Some(&initial),
                fixture.backend("sys.exit(1)")?,
                cancel(),
                Arc::new(|_, _| {})
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(path)?, previous);
    assert_eq!(
        fixture
            .tracker
            .mele_deployment(&fixture.game.id)
            .await?
            .context("Missing state")?
            .generation,
        initial.generation
    );
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned helper and supplied game caches; uses disposable copies only"]
async fn deploys_real_global_shader_caches_and_restores_all_three_games() -> Result<()> {
    let settings: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../helpers/mele/toolchain.json"
    ))?;
    let sdk = settings["sdk"]["version"].as_str().context("Missing SDK")?;
    let backend = helper::Backend {
        runtime: PathBuf::from(format!("/build/mele/sdk-{sdk}/dotnet")),
        assembly: PathBuf::from(
            "/build/mele/source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll",
        ),
        native_library: PathBuf::from(
            "/build/mele/source/Deployd.Mele/bin/Release/net10.0/libdeployd_oodle.so",
        ),
    };
    for (game, number) in [(Target::Le1, 1), (Target::Le2, 2), (Target::Le3, 3)] {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "modTesting/Mass Effect Legendary Edition (Game)/Game/ME{number}/BioGame/{}",
            m3gs::TARGET
        ));
        let original = fs::read(&source)?;
        let fixture = Fixture::with_game(game, |root| {
            fs::write(root.join("BioGame").join(m3gs::TARGET), &original)?;
            Ok(())
        })
        .await?;
        lifecycle(&fixture, backend.clone()).await?;
        assert_eq!(fs::read(source)?, original);
    }
    Ok(())
}
