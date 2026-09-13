use super::*;
use crate::core::game::mass_effect::components;

// @variants: both
#[tokio::test]
#[ignore = "Uses supplied ALOT LE2 and downloads pinned runtimes; deployment is confined to a disposable fixture"]
async fn deploys_alot_rebuilds_rejects_broken_tfc_combinations_and_restores() -> Result<()> {
    alot_lifecycle(Target::Le2, "ALotofTextures(ALOT)forLE2_2021.1", 11_121_088).await
}

// @variants: both
#[tokio::test]
#[ignore = "Uses supplied ALOT LE1 and downloads pinned runtimes; deployment is confined to a disposable fixture"]
async fn deploys_le1_alot_rebuilds_rejects_broken_tfc_combinations_and_restores() -> Result<()> {
    alot_lifecycle(
        Target::Le1,
        "ALotofTextures(ALOT)forLE1_2021.1.3",
        111_108_208,
    )
    .await
}

async fn alot_lifecycle(target: Target, folder: &str, btp_size: u64) -> Result<()> {
    let fixture = Fixture::with_game(target, |game| {
        fs::write(game.join(components::BINK), b"original game library")?;
        Ok(())
    })
    .await?;
    let mut selections = Vec::new();
    components::texture_runtime(&mut selections, target, true);
    components::test_cache::seed(&fixture.data, &selections).await?;
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("modTesting")
        .join(folder);
    let inspected = {
        let source = source.clone();
        tokio::task::spawn_blocking(move || PackagePlan::inspect(&source, Some(target))).await??
    };
    let stored = sources::retain_in(
        source,
        inspected,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let recipe = fixture.recipe(vec![stored.selection()]);
    eprintln!("Deploying supplied ALOT with pinned texture runtime");
    let installed = fixture.deploy(recipe, None).await?;
    let persisted = fixture
        .tracker
        .mele_recipe(&fixture.game.id, &fixture.profile)
        .await?
        .context("missing persisted recipe")?;
    let (component, runtime) = match target {
        Target::Le1 => (
            components::Component::Le1TextureOverride,
            "LE1TextureOverride-v3.asi",
        ),
        Target::Le2 => (
            components::Component::Le2TextureOverride,
            "LE2TextureOverride-v3.asi",
        ),
        Target::Le3 => (
            components::Component::Le3TextureOverride,
            "LE3TextureOverride-v3.asi",
        ),
    };
    assert!(
        persisted
            .components
            .iter()
            .any(|selection| selection.component == component)
    );
    let btp = "BioGame/DLC/DLC_MOD_ALOT/CombinedTextureOverrides.btp";
    assert_eq!(fs::metadata(fixture.game.path.join(btp))?.len(), btp_size);
    eprintln!("Rebuilding ALOT from persisted recipe");
    let rebuilt = fixture.deploy(persisted.clone(), Some(&installed)).await?;
    assert_eq!(rebuilt.files, installed.files);

    let (overlay, _) = fixture.package(b"invalid texture cache", "")?;
    let manifest = fs::read_to_string(overlay.join("moddesc.ini"))?
        .replace("destdirs=DLC_MOD_Test", "destdirs=DLC_MOD_ALOT")
        .replace("Test.pcc", "Textures_DLC_MOD_ALOT.tfc");
    fs::write(overlay.join("moddesc.ini"), manifest)?;
    fs::rename(
        overlay.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"),
        overlay.join("DLC_MOD_Test/CookedPCConsole/Textures_DLC_MOD_ALOT.tfc"),
    )?;
    let inspected = PackagePlan::inspect(&overlay, Some(target))?;
    let overlay = sources::retain_in(
        overlay,
        inspected,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let mut broken = persisted.clone();
    broken.packages.push(overlay.selection());
    eprintln!("Rejecting incompatible TFC replacement before publication");
    let error = fixture
        .deploy(broken, Some(&rebuilt))
        .await
        .err()
        .context("broken combination was accepted")?;
    assert!(format!("{error:#}").contains("TFC"), "{error:#}");
    assert_eq!(fs::read(fixture.game.path.join(ENGINE))?, b"original");
    assert_eq!(
        fixture
            .tracker
            .mele_recipe(&fixture.game.id, &fixture.profile)
            .await?,
        Some(persisted)
    );

    eprintln!("Removing ALOT and restoring the original library");
    let restored = fixture
        .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
        .await?;
    assert!(restored.files.is_empty());
    assert!(!fixture.game.path.join(btp).exists());
    assert!(
        !fixture
            .game
            .path
            .join("Binaries/Win64/ASI")
            .join(runtime)
            .exists()
    );
    assert_eq!(
        fs::read(fixture.game.path.join(components::BINK))?,
        b"original game library"
    );
    assert!(!fixture.game.path.join(components::ORIGINAL).exists());
    Ok(())
}
