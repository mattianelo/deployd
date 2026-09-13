use std::path::Path;

use sha2::{Digest, Sha256};

use super::*;

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("modTesting")
}

fn copy_input(
    source: &Path,
    destination: &Path,
    identities: &mut Vec<(PathBuf, String)>,
) -> Result<()> {
    let bytes = fs::read(source)?;
    identities.push((
        source.to_path_buf(),
        format!("{:x}", Sha256::digest(&bytes)),
    ));
    fs::create_dir_all(destination.parent().context("missing input parent")?)?;
    fs::write(destination, bytes)?;
    Ok(())
}

fn backend() -> Result<helper::Backend> {
    let settings: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../../helpers/mele/toolchain.json"
    ))?;
    let sdk = settings["sdk"]["version"].as_str().context("missing SDK")?;
    let root = PathBuf::from("/build/mele/source/Deployd.Mele/bin/Release/net10.0");
    let backend = helper::Backend {
        runtime: PathBuf::from(format!("/build/mele/sdk-{sdk}/dotnet")),
        assembly: root.join("Deployd.Mele.dll"),
        native_library: root.join("libdeployd_oodle.so"),
    };
    for path in [&backend.runtime, &backend.assembly, &backend.native_library] {
        anyhow::ensure!(
            path.is_file(),
            "Build the pinned helper through ./check.sh mele build first"
        );
    }
    Ok(backend)
}

fn assert_restored(fixture: &Fixture) -> Result<()> {
    for file in &fixture.baseline.files {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(fixture.game.path.join(&file.relative))?)
            ),
            file.sha256,
            "{}",
            file.relative
        );
    }
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper, supplied LE1 corpus, and space for independent package copies"]
async fn deploys_supplied_le1_playtest_mods_with_community_patch_and_restores_originals()
-> Result<()> {
    let backend = backend()?;
    let mut inputs = Vec::new();
    for (name, version) in [
        ("LE1 Community Patch", "2.0"),
        ("Galaxy Map Trackers (LE1)", "1.3"),
        ("Mission Timings", "1.0"),
        ("Pinnacle Station DLC", "2.2.1"),
        ("Saren Stages", "3.1"),
    ] {
        eprintln!("Inspecting {name}");
        let source = corpus().join(name);
        let inspected = {
            let source = source.clone();
            tokio::task::spawn_blocking(move || PackagePlan::inspect(&source, Some(Target::Le1)))
                .await??
        };
        assert_eq!(inspected.manifest.version, version);
        eprintln!("{name}: {}", inspected.source_sha256);
        inputs.push((source, inspected));
    }
    let mut names: BTreeSet<String> = helper::jobs::BASES
        .iter()
        .map(|name| (*name).into())
        .collect();
    names.extend(["PlotManager.pcc".into(), "Coalesced_INT.bin".into()]);
    for (_, plan) in &inputs {
        for merge in &plan.m3m {
            names.extend(
                merge
                    .files
                    .iter()
                    .flat_map(|file| file.target_candidates.iter().cloned()),
            );
        }
        if let Some(tlk) = &plan.embedded_tlk {
            names.extend(tlk.updates.iter().map(|update| update.package.clone()));
        }
        for table in &plan.m3da {
            names.extend(table.merges.iter().map(|merge| merge.target.clone()));
        }
    }
    let game_source = corpus().join("Mass Effect Legendary Edition (Game)/Game/ME1");
    let mut identities = Vec::new();
    eprintln!("Preparing disposable game inputs");
    let mut fixture = Fixture::with_game(Target::Le1, |game| {
        fs::remove_dir_all(game.join("BioGame/CookedPCConsole"))?;
        fs::create_dir_all(game.join("BioGame/CookedPCConsole"))?;
        for name in names {
            let relative = format!("BioGame/CookedPCConsole/{name}");
            if game_source.join(&relative).is_file() {
                copy_input(
                    &game_source.join(&relative),
                    &game.join(relative),
                    &mut identities,
                )?;
            }
        }
        let codec = "Binaries/Win64/oo2core_8_win64.dll";
        copy_input(&game_source.join(codec), &game.join(codec), &mut identities)?;
        Ok(())
    })
    .await?;
    let mut packages = Vec::new();
    for (source, plan) in &inputs {
        eprintln!("Retaining {}", plan.manifest.name);
        packages.push(
            sources::retain_in(
                source.clone(),
                plan.clone(),
                None,
                fixture.data.clone(),
                cancel(),
                Arc::new(|_, _| {}),
            )
            .await?
            .selection(),
        );
    }
    let progress: Progress = Arc::new(|done, total| {
        if done % 100 == 0 {
            eprintln!("Deployment progress: {done}/{total}");
        }
    });
    let recipe = fixture.merge_recipe(packages.clone());
    let result = async {
        eprintln!("Deploying the combined LE1 recipe");
        let installed = fixture
            .deploy_merges(
                recipe.clone(),
                None,
                backend.clone(),
                cancel(),
                progress.clone(),
            )
            .await?;
        assert_eq!(installed.recipe.as_ref(), Some(&recipe));
        for dlc in [
            "DLC_MOD_LE1CP",
            "DLC_MOD_GalaxyMapTrackers",
            "DLC_MOD_MissionTimings",
            "DLC_MOD_Vegas",
            "DLC_MOD_Saren_Stages",
        ] {
            assert!(
                fixture
                    .game
                    .path
                    .join(format!("BioGame/DLC/{dlc}/AutoLoad.ini"))
                    .is_file(),
                "{dlc}"
            );
        }
        for relative in [
            ENGINE,
            "BioGame/CookedPCConsole/PlotManager.pcc",
            "BioGame/CookedPCConsole/Coalesced_INT.bin",
        ] {
            let generated = installed
                .files
                .iter()
                .find(|file| file.relative == relative)
                .context("missing generated output")?;
            let original = fixture
                .baseline
                .files
                .iter()
                .find(|file| file.relative == relative)
                .context("missing original")?;
            assert_ne!(generated.sha256, original.sha256, "{relative}");
        }
        fixture.tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            fixture.temp.path().join("tracker.db").display()
        ))
        .await?
        .tracker;
        eprintln!("Rebuilding after reopening the database");
        let rebuilt = fixture
            .deploy_merges(
                recipe,
                Some(&installed),
                backend.clone(),
                cancel(),
                progress.clone(),
            )
            .await?;
        assert_eq!(installed.files, rebuilt.files);
        eprintln!("Removing the additional mods while retaining Community Patch");
        let remaining = fixture
            .deploy_merges(
                fixture.merge_recipe(vec![packages[0].clone()]),
                Some(&rebuilt),
                backend,
                cancel(),
                progress,
            )
            .await?;
        for (_, plan) in inputs.iter().skip(1) {
            for mapping in &plan.files {
                assert!(
                    !fixture
                        .game
                        .path
                        .join("BioGame")
                        .join(&mapping.destination)
                        .exists(),
                    "{}",
                    mapping.destination
                );
            }
        }
        let plot = |state: &journal::State| {
            state
                .files
                .iter()
                .find(|file| file.relative == "BioGame/CookedPCConsole/PlotManager.pcc")
                .map(|file| file.sha256.clone())
        };
        assert_ne!(plot(&remaining), plot(&rebuilt));
        eprintln!("Restoring every original game file");
        let restored = fixture
            .deploy(fixture.recipe(Vec::new()), Some(&remaining))
            .await?;
        assert!(restored.files.is_empty());
        assert_restored(&fixture)?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    for (path, identity) in identities {
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(path)?)), identity);
    }
    for (source, plan) in &inputs {
        plan.verify_sources(source)?;
    }
    result
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the supplied Saren package, clean LE1 configuration, and pinned Linux helper"]
async fn deploys_supplied_saren_assets_and_config_with_a_versioned_provider() -> Result<()> {
    let backend = backend()?;
    let source = corpus().join("Saren Stages");
    let plan = PackagePlan::inspect(&source, Some(Target::Le1))?;
    assert_eq!(plan.manifest.version, "3.1");
    let game_source = corpus().join("Mass Effect Legendary Edition (Game)/Game/ME1");
    let config = "BioGame/CookedPCConsole/Coalesced_INT.bin";
    let mut identities = Vec::new();
    let fixture = Fixture::with_game(Target::Le1, |game| {
        fs::remove_file(game.join("BioGame/CookedPCConsole/Coalesced_FRA.bin"))?;
        copy_input(
            &game_source.join(config),
            &game.join(config),
            &mut identities,
        )
    })
    .await?;
    // A small provider isolates dependency resolution from the already-tested Community Patch transformations.
    let (provider_root, _) = fixture.package(b"provider", "")?;
    let manifest = provider_root.join("moddesc.ini");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)?
            .replace("modver=1.0", "modver=2.0")
            .replace("destdirs=DLC_MOD_Test", "destdirs=DLC_MOD_LE1CP"),
    )?;
    let provider_plan = PackagePlan::inspect(&provider_root, None)?;
    let provider = sources::retain_in(
        provider_root,
        provider_plan,
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection();
    let package = sources::retain_in(
        source.clone(),
        plan.clone(),
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection();
    let installed = fixture
        .deploy_merges(
            fixture.merge_recipe(vec![provider.clone(), package]),
            None,
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    for mapping in &plan.files {
        if mapping.destination.ends_with(".m3cd") {
            continue;
        }
        let file = plan
            .sources
            .iter()
            .find(|file| file.relative == mapping.source)
            .context("missing Saren input")?;
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(fs::read(
                    fixture.game.path.join("BioGame").join(&mapping.destination)
                )?)
            ),
            file.sha256
        );
    }
    let output = fs::read(fixture.game.path.join(config))?;
    assert_ne!(format!("{:x}", Sha256::digest(&output)), identities[0].1);
    let restored = fixture
        .deploy(fixture.recipe(vec![provider]), Some(&installed))
        .await?;
    assert!(
        !restored
            .files
            .iter()
            .any(|file| file.relative.contains("DLC_MOD_Saren_Stages"))
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(fs::read(fixture.game.path.join(config))?)
        ),
        identities[0].1
    );
    assert_eq!(PackagePlan::inspect(&source, Some(Target::Le1))?, plan);
    for (source, hash) in identities {
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(source)?)), hash);
    }
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the supplied LE2 Sheploo package, clean SFXGame package, and pinned Linux helper"]
async fn deploys_supplied_le2_sheploo_rebuilds_and_restores_originals() -> Result<()> {
    let backend = backend()?;
    let source = corpus().join("Sheploo Appearance Consistency Project");
    let plan = PackagePlan::inspect(&source, Some(Target::Le2))?;
    assert_eq!(plan.manifest.version, "2.0");
    let game_source = corpus().join("Mass Effect Legendary Edition (Game)/Game/ME2");
    let target = "BioGame/CookedPCConsole/SFXGame.pcc";
    let mut identities = Vec::new();
    let mut fixture = Fixture::with_game(Target::Le2, |game| {
        for relative in [target, "Binaries/Win64/oo2core_8_win64.dll"] {
            copy_input(
                &game_source.join(relative),
                &game.join(relative),
                &mut identities,
            )?;
        }
        Ok(())
    })
    .await?;
    let package = sources::retain_in(
        source.clone(),
        plan.clone(),
        None,
        fixture.data.clone(),
        cancel(),
        Arc::new(|_, _| {}),
    )
    .await?
    .selection();
    let recipe = fixture.merge_recipe(vec![package]);
    let installed = fixture
        .deploy_merges(
            recipe.clone(),
            None,
            backend.clone(),
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_ne!(
        format!(
            "{:x}",
            Sha256::digest(fs::read(fixture.game.path.join(target))?)
        ),
        identities[0].1
    );
    fixture.tracker = Tracker::open(&format!(
        "sqlite://{}?mode=rwc",
        fixture.temp.path().join("tracker.db").display()
    ))
    .await?
    .tracker;
    let rebuilt = fixture
        .deploy_merges(
            recipe,
            Some(&installed),
            backend,
            cancel(),
            Arc::new(|_, _| {}),
        )
        .await?;
    assert_eq!(installed.files, rebuilt.files);
    let restored = fixture
        .deploy(fixture.recipe(Vec::new()), Some(&rebuilt))
        .await?;
    assert!(restored.files.is_empty());
    assert_restored(&fixture)?;
    assert_eq!(PackagePlan::inspect(&source, Some(Target::Le2))?, plan);
    for (source, hash) in identities {
        assert_eq!(format!("{:x}", Sha256::digest(fs::read(source)?)), hash);
    }
    Ok(())
}
