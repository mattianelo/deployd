use std::path::Path;

use sha2::{Digest, Sha256};

use super::*;

fn copy(source: &Path, destination: &Path, identities: &mut Vec<(PathBuf, String)>) -> Result<()> {
    let bytes = fs::read(source)?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    fs::create_dir_all(destination.parent().context("missing corpus parent")?)?;
    fs::write(destination, bytes)?;
    identities.push((source.to_path_buf(), hash));
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper, supplied game/mod corpus, and space for independent full-package copies"]
async fn installs_complete_community_patch_recipe_and_restores_game_files() -> Result<()> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/modTesting/LE1 Community Patch");
    eprintln!("Inspecting the complete Community Patch package");
    let inspected = {
        let source = source.clone();
        tokio::task::spawn_blocking(move || PackagePlan::inspect(&source, None)).await??
    };
    assert_eq!(
        inspected.source_sha256,
        "6515f032e27a1b686e6a1eeb69143cb52cb792ecdf2eb2d8566e67846e6f04d6"
    );
    let mut names: BTreeSet<String> = helper::jobs::BASES
        .iter()
        .map(|name| (*name).into())
        .collect();
    for plan in &inspected.m3m {
        names.extend(
            plan.files
                .iter()
                .flat_map(|file| file.target_candidates.iter().cloned()),
        );
    }
    names.extend(
        inspected
            .embedded_tlk
            .as_ref()
            .context("missing TLK corpus")?
            .updates
            .iter()
            .map(|update| update.package.clone()),
    );
    names.extend(["PlotManager.pcc".into(), "Coalesced_INT.bin".into()]);
    let game_source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs/modTesting/Mass Effect Legendary Edition (Game)/Game/ME1");
    let mut identities = Vec::new();
    eprintln!("Preparing disposable game files");
    let fixture = Fixture::with_game(Target::Le1, |game| {
        fs::remove_dir_all(game.join("BioGame/CookedPCConsole"))?;
        fs::create_dir_all(game.join("BioGame/CookedPCConsole"))?;
        for name in names {
            let relative = format!("BioGame/CookedPCConsole/{name}");
            if game_source.join(&relative).is_file() {
                copy(
                    &game_source.join(&relative),
                    &game.join(relative),
                    &mut identities,
                )?;
            }
        }
        copy(
            &game_source.join("Binaries/Win64/oo2core_8_win64.dll"),
            &game.join("Binaries/Win64/oo2core_8_win64.dll"),
            &mut identities,
        )?;
        Ok(())
    })
    .await?;
    eprintln!("Retaining immutable Community Patch sources");
    let stored = sources::retain_in(
        source.clone(),
        inspected.clone(),
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let recipe = fixture.merge_recipe(vec![stored.selection()]);
    let settings: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../helpers/mele/toolchain.json"
    ))?;
    let sdk = settings["sdk"]["version"].as_str().context("missing SDK")?;
    let backend = helper::Backend {
        runtime: PathBuf::from(format!("/build/mele/sdk-{sdk}/dotnet")),
        assembly: PathBuf::from(
            "/build/mele/source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll",
        ),
        native_library: PathBuf::from(
            "/build/mele/source/Deployd.Mele/bin/Release/net10.0/libdeployd_oodle.so",
        ),
    };
    let result = async {
        eprintln!("Replaying Community Patch files, M3M, TLK, and target merges");
        let progress: Progress = Arc::new(|done, total| {
            if done % 100 == 0 {
                eprintln!("Community Patch rebuild progress: {done}/{total}");
            }
        });
        let installed = fixture
            .deploy_merges(
                recipe.clone(),
                None,
                backend.clone(),
                cancel(),
                progress.clone(),
            )
            .await?;
        assert_eq!(installed.recipe, Some(recipe.clone()));
        assert!(installed.files.len() >= inspected.files.len());
        assert_ne!(
            fs::read(fixture.game.path.join(ENGINE))?,
            fs::read(game_source.join(ENGINE))?
        );
        eprintln!("Rebuilding the same Community Patch recipe");
        let rebuilt = fixture
            .deploy_merges(recipe, Some(&installed), backend, cancel(), progress)
            .await?;
        eprintln!("Removing Community Patch and restoring the baseline");
        fixture
            .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
            .await?;
        for file in &fixture.baseline.files {
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(fixture.game.path.join(&file.relative))?)
                ),
                file.sha256
            );
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    for (path, hash) in identities {
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(path)?)), hash);
    }
    inspected.verify_sources(&source)?;
    result
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper and supplied disposable game/mod corpus"]
async fn journals_real_community_patch_target_merges_and_restores_the_baseline() -> Result<()> {
    let source = Path::new("docs/modTesting/Mass Effect Legendary Edition (Game)/Game/ME1");
    let mut identities = Vec::new();
    let fixture = Fixture::with_game(Target::Le1, |game| {
        for name in ["Coalesced_INT.bin", "PlotManager.pcc"]
            .into_iter()
            .chain(helper::jobs::BASES.iter().copied())
        {
            let relative = format!("BioGame/CookedPCConsole/{name}");
            copy(
                &source.join(&relative),
                &game.join(relative),
                &mut identities,
            )?;
        }
        fs::remove_file(game.join("BioGame/CookedPCConsole/Coalesced_FRA.bin"))?;
        copy(
            &source.join("Binaries/Win64/oo2core_8_win64.dll"),
            &game.join("Binaries/Win64/oo2core_8_win64.dll"),
            &mut identities,
        )?;
        Ok(())
    })
    .await?;
    assert_eq!(
        identities
            .iter()
            .find(|(path, _)| path.ends_with(ENGINE))
            .context("missing engine identity")?
            .1,
        "296308fba3fe6294f8ed4eba5796fcc54e75e51837ff9cc261e1151124f4ad13"
    );
    let package = fixture.temp.path().join("package");
    let input = Path::new("docs/modTesting/LE1 Community Patch");
    for name in [
        "AutoLoad.ini",
        "CookedPCConsole/DLC_MOD_LE1CP-2DAMerge.m3da",
        "CookedPCConsole/DLC_MOD_LE1CP_2DA.pcc",
        "CookedPCConsole/ConfigDelta-ModSettingsMenu.m3cd",
        "CookedPCConsole/ConfigDelta-PCOptions_Persistent.m3cd",
        "CookedPCConsole/PlotManagerUpdate.pmu",
    ] {
        let relative = format!("DLC_MOD_LE1CP/{name}");
        copy(
            &input.join(&relative),
            &package.join(relative),
            &mut identities,
        )?;
    }
    fs::write(
        package.join("moddesc.ini"),
        "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE1\nmodname=Target merge corpus\nmodver=2.0\nmoddev=Test\nmoddesc=Target transformations only\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_LE1CP\ndestdirs=DLC_MOD_LE1CP\n",
    )?;
    let inspected = PackagePlan::inspect(&package, None)?;
    let stored = sources::retain_in(
        package,
        inspected,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?;
    let recipe = fixture.merge_recipe(vec![stored.selection()]);
    let settings: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../helpers/mele/toolchain.json"
    ))?;
    let sdk = settings["sdk"]["version"]
        .as_str()
        .context("missing SDK version")?;
    let backend = helper::Backend {
        runtime: PathBuf::from(format!("/build/mele/sdk-{sdk}/dotnet")),
        assembly: PathBuf::from(
            "/build/mele/source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll",
        ),
        native_library: PathBuf::from(
            "/build/mele/source/Deployd.Mele/bin/Release/net10.0/libdeployd_oodle.so",
        ),
    };
    let result = async {
        let installed = fixture
            .deploy_merges(
                recipe.clone(),
                None,
                backend.clone(),
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        for name in ["Engine.pcc", "Coalesced_INT.bin", "PlotManager.pcc"] {
            let relative = format!("BioGame/CookedPCConsole/{name}");
            let original = fixture
                .baseline
                .files
                .iter()
                .find(|file| file.relative == relative)
                .context("missing baseline")?;
            let generated = installed
                .files
                .iter()
                .find(|file| file.relative == relative)
                .context("missing generated file")?;
            assert_ne!(original.sha256, generated.sha256);
        }
        assert!(
            !installed
                .files
                .iter()
                .any(|file| file.relative.ends_with("/Core.pcc"))
        );
        let rebuilt = fixture
            .deploy_merges(
                recipe.clone(),
                Some(&installed),
                backend,
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?;
        let removed = fixture
            .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
            .await?;
        assert!(removed.files.is_empty());
        for file in &fixture.baseline.files {
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(fixture.game.path.join(&file.relative))?)
                ),
                file.sha256
            );
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    for (path, hash) in identities {
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(path)?)), hash);
    }
    result
}
