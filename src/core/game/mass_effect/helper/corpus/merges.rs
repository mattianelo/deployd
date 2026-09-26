use super::*;
use crate::core::game::mass_effect::m3m as format_m3m;
use protocol::{Contribution, M3mKind};

fn record(corpus: &Corpus, output: &ValidatedOutput, name: &str) -> Result<()> {
    for file in &output.files {
        let path = corpus._root.path().join(name).join(&file.path);
        fs::create_dir_all(path.parent().context("Missing reference parent")?)?;
        fs::copy(output.root().join(&file.path), path)?;
    }
    Ok(())
}

async fn reference(corpus: &Corpus, flag: &str) -> Result<()> {
    let output = tokio::time::timeout(Duration::from_secs(120), Command::new(&corpus.backend.runtime)
        .arg("/build/mele/source/TransformationTests/bin/Release/net10.0/TransformationTests.dll")
        .arg(flag).arg(corpus._root.path()).env("DOTNET_EnableDiagnostics", "0")
        .stdin(Stdio::null()).kill_on_drop(true).output()).await??;
    ensure!(
        output.status.success(),
        "Pinned reference comparison failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

async fn target_merges(config: bool) -> Result<()> {
    let mut corpus = Corpus::new()?;
    let specification = if config {
        (
            "CookedPCConsole/Coalesced_INT.bin",
            1464542,
            "faea7471d285c72ccf36aa8263d1f04fe9e7a53bb0b708488ae552e34a7c4cd9",
        )
    } else {
        BASES[1]
    };
    let root = Path::new(GAME).join("BioGame");
    let original = corpus.copy(
        &root,
        specification,
        &corpus
            .inputs
            .original
            .clone()
            .context("Missing original root")?,
        specification.0,
    )?;
    let current = corpus.copy(
        &root,
        specification,
        &corpus.inputs.candidate.clone(),
        specification.0,
    )?;
    let mut contributions = Vec::new();
    let manifests: Vec<_> = if config {
        vec![
            (
                "ConfigDelta-ModSettingsMenu.m3cd",
                959,
                "e07a1685ee49f56ebf2f06109f38fd25109a9b3c741221268cbe98686d173410",
            ),
            (
                "ConfigDelta-PCOptions_Persistent.m3cd",
                465,
                "21d2eb484cea9107ad0a8299d694ccc8049678319c0400548669adff3523cad2",
            ),
        ]
    } else {
        vec![(
            "DLC_MOD_LE1CP-2DAMerge.m3da",
            395,
            "aafd92789ea505833fa5d407058636c09afb9b75044776bb1172ee3e20edc7f1",
        )]
    };
    for (name, size, sha256) in manifests {
        let source = format!("DLC_MOD_LE1CP/CookedPCConsole/{name}");
        let destination = format!("DLC/{source}");
        let manifest = corpus.copy(
            Path::new(MOD),
            (&source, size, sha256),
            &corpus.inputs.candidate.clone(),
            &destination,
        )?;
        let mut packages = Vec::new();
        if !config {
            let source = "DLC_MOD_LE1CP/CookedPCConsole/DLC_MOD_LE1CP_2DA.pcc";
            packages.push(corpus.copy(
                Path::new(MOD),
                (
                    source,
                    4804,
                    "f013a55a71da9377fbb38069e08fccd373b29dbada6c1a84cbd0decc813ec8f9",
                ),
                &corpus.inputs.candidate.clone(),
                &format!("DLC/{source}"),
            )?);
        }
        contributions.push(Contribution {
            dlc: "DLC_MOD_LE1CP".into(),
            mount: 5,
            manifest,
            packages,
        });
    }
    contributions.reverse();
    let targets = vec![TargetPackage { original, current }];
    let mut job = if config {
        Job::Config {
            targets,
            contributions,
        }
    } else {
        Job::Tables {
            targets,
            contributions,
        }
    };
    for phase in ["merged", "reapplied", "removed"] {
        if phase == "removed" {
            match &mut job {
                Job::Config { contributions, .. } | Job::Tables { contributions, .. } => {
                    contributions.clear()
                }
                _ => unreachable!(),
            }
        }
        let result = corpus.run(job.clone()).await;
        corpus.verify()?;
        let output = result?;
        record(&corpus, &output, phase)?;
        match &mut job {
            Job::Config { targets, .. } | Job::Tables { targets, .. } => {
                targets[0].current = output.files[0].clone();
                fs::copy(
                    output.root().join(specification.0),
                    corpus.inputs.candidate.join(specification.0),
                )?;
            }
            _ => unreachable!(),
        }
    }
    reference(
        &corpus,
        if config {
            "--community-patch-config"
        } else {
            "--community-patch"
        },
    )
    .await?;
    corpus.verify()?;
    assert_eq!(fs::read_dir(&corpus.staging)?.count(), 0);
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper/reference tests and supplied disposable corpus"]
async fn supervises_real_community_patch_tables_with_reference_and_removal() -> Result<()> {
    target_merges(false).await
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper/reference tests and supplied disposable corpus"]
async fn supervises_real_community_patch_config_with_reference_and_removal() -> Result<()> {
    target_merges(true).await
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper/reference tests and supplied disposable corpus"]
async fn supervises_real_community_patch_m3m_prepared_in_rust() -> Result<()> {
    let mut corpus = Corpus::new()?;
    let package = corpus._root.path().join("package");
    let preparation = corpus._root.path().join("preparation");
    fs::create_dir(&package)?;
    fs::create_dir(&preparation)?;
    let mut plans = Vec::new();
    for (name, size, hash) in MERGES {
        let source = format!("MergeMods/{name}.m3m");
        let input = corpus.copy(Path::new(MOD), (&source, *size, hash), &package, &source)?;
        plans.push(format_m3m::inspect(
            &package,
            &SourceFile {
                relative: input.path,
                size: input.size,
                sha256: input.sha256,
            },
            Target::Le1,
        )?);
    }
    let mut packages = Vec::new();
    for specification in BASES[..6].iter().chain([
        &(
            "CookedPCConsole/Startup_INT.pcc",
            45198425,
            "10e71f9eb7883789942f8f464693419838f4d8f7873de1ae0f2aa3fd84c0a860",
        ),
        &(
            "CookedPCConsole/BIOC_Materials.pcc",
            13468055,
            "a07c30f8f3351a8e9d9c4161dd2894dd62f5db81298558ef749dc7d63cd9a2ae",
        ),
    ]) {
        let original = corpus.copy(
            &Path::new(GAME).join("BioGame"),
            *specification,
            &corpus.inputs.original.clone().context("Missing original")?,
            specification.0,
        )?;
        let current = corpus.copy(
            &Path::new(GAME).join("BioGame"),
            *specification,
            &corpus.inputs.candidate.clone(),
            specification.0,
        )?;
        packages.push(TargetPackage { original, current });
    }
    let prepared = m3m::prepare(
        m3m::Sources {
            package,
            original: corpus.inputs.original.clone().context("Missing original")?,
            candidate: corpus.inputs.candidate.clone(),
            packages,
            plans,
        },
        preparation,
        Arc::new(AtomicBool::new(false)),
    )
    .await?;
    let Job::M3m { jobs, .. } = &prepared.job else {
        unreachable!();
    };
    assert_eq!(jobs.len(), 37);
    for (kind, count) in [
        (M3mKind::Asset, 11),
        (M3mKind::Class, 1),
        (M3mKind::Function, 14),
        (M3mKind::Member, 11),
    ] {
        assert_eq!(jobs.iter().filter(|job| job.kind == kind).count(), count);
    }
    corpus.inputs.candidate = prepared.root().to_path_buf();
    let result = corpus.run(prepared.job.clone()).await;
    corpus.verify()?;
    let output = result?;
    record(&corpus, &output, "merged")?;
    let verification = corpus._root.path().join("verification");
    fs::create_dir(&verification)?;
    let mut request: serde_json::Value =
        serde_json::from_slice(&fs::read(output.stage.path().join("request.json"))?)?;
    request["output_root"] = serde_json::to_value(&verification)?;
    fs::write(
        corpus._root.path().join("semantic-request.json"),
        serde_json::to_vec(&request)?,
    )?;
    let previous = corpus._root.path().join("previous");
    let mut repeated = prepared.job.clone();
    for input in prepared.job.inputs() {
        let destination = previous.join(&input.path);
        fs::create_dir_all(destination.parent().context("Missing previous parent")?)?;
        fs::copy(prepared.root().join(&input.path), destination)?;
    }
    if let Job::M3m { targets, .. } = &mut repeated {
        for target in targets {
            target.current = output
                .files
                .iter()
                .find(|file| file.path == target.current.path)
                .context("Missing output")?
                .clone();
            fs::copy(
                output.root().join(&target.current.path),
                previous.join(&target.current.path),
            )?;
        }
    }
    corpus.inputs.candidate = previous;
    let again = corpus.run(repeated).await?;
    record(&corpus, &again, "reapplied")?;
    reference(&corpus, "--community-patch-m3m").await?;
    corpus.inputs.candidate = prepared.root().to_path_buf();
    let mut failed = prepared.job.clone();
    if let Job::M3m { jobs, .. } = &mut failed {
        jobs.last_mut().context("Missing final job")?.entry = "MissingExport".into();
    }
    assert!(corpus.run(failed).await.is_err());
    corpus.verify()?;
    for input in prepared.job.inputs() {
        files::identity(prepared.root(), input, &tests::control())?;
    }
    drop((output, again));
    assert_eq!(fs::read_dir(&corpus.staging)?.count(), 0);
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the Linux helper/reference tests and supplied LE3 game and Community Patch"]
async fn supervises_supplied_le3_options_scaling_merge() -> Result<()> {
    let game = Path::new("modTesting/Mass Effect Legendary Edition/Game/ME3");
    let mut corpus = Corpus::for_game(game)?;
    let package = corpus._root.path().join("package");
    let preparation = corpus._root.path().join("preparation");
    fs::create_dir(&preparation)?;
    let source = corpus.copy(
        Path::new("modTesting/LE3 Community Framework and Patch-13-1-7-9-1777095412"),
        (
            "MergeMods/optionsScalingFix.m3m",
            41577,
            "57d0bfe4d20e44122e76f8aac18bdd508ad50d8dd7dba665c9218bebf69eda6f",
        ),
        &package,
        "MergeMods/optionsScalingFix.m3m",
    )?;
    let plan = format_m3m::inspect(
        &package,
        &SourceFile {
            relative: source.path,
            size: source.size,
            sha256: source.sha256,
        },
        Target::Le3,
    )?;
    let specification = (
        "CookedPCConsole/Startup.pcc",
        13595946,
        "bd083d5a3cb9efe864350f7efa13084ffc4dbd6d77fd628c8fb57f8d59a44962",
    );
    let original_root = corpus
        .inputs
        .original
        .clone()
        .context("Missing original root")?;
    let original = corpus.copy(
        &game.join("BioGame"),
        specification,
        &original_root,
        specification.0,
    )?;
    let current = corpus.copy(
        &game.join("BioGame"),
        specification,
        &corpus.inputs.candidate.clone(),
        specification.0,
    )?;
    let prepared = m3m::prepare(
        m3m::Sources {
            package,
            original: original_root,
            candidate: corpus.inputs.candidate.clone(),
            packages: vec![TargetPackage { original, current }],
            plans: vec![plan],
        },
        preparation,
        Arc::new(AtomicBool::new(false)),
    )
    .await?;
    corpus.inputs.candidate = prepared.root().to_path_buf();
    let result = corpus.run(prepared.job.clone()).await;
    corpus.verify()?;
    let output = result?;
    assert_eq!(output.files.len(), 1);
    assert_eq!(output.files[0].path, specification.0);
    assert_ne!(output.files[0].sha256, specification.2);
    record(&corpus, &output, "merged")?;
    let verification = corpus._root.path().join("verification");
    fs::create_dir(&verification)?;
    let mut request: serde_json::Value =
        serde_json::from_slice(&fs::read(output.stage.path().join("request.json"))?)?;
    request["output_root"] = serde_json::to_value(&verification)?;
    fs::write(
        corpus._root.path().join("semantic-request.json"),
        serde_json::to_vec(&request)?,
    )?;
    let previous = corpus._root.path().join("previous");
    for input in prepared.job.inputs() {
        let destination = previous.join(&input.path);
        fs::create_dir_all(destination.parent().context("Missing previous parent")?)?;
        fs::copy(prepared.root().join(&input.path), destination)?;
    }
    let mut repeated = prepared.job.clone();
    if let Job::M3m { targets, .. } = &mut repeated {
        targets[0].current = output.files[0].clone();
    }
    fs::copy(
        output.root().join(specification.0),
        previous.join(specification.0),
    )?;
    corpus.inputs.candidate = previous;
    let again = corpus.run(repeated).await?;
    record(&corpus, &again, "reapplied")?;
    reference(&corpus, "--community-patch-m3m").await?;
    corpus.verify()?;
    for input in prepared.job.inputs() {
        files::identity(prepared.root(), input, &tests::control())?;
    }
    drop((output, again));
    assert_eq!(fs::read_dir(&corpus.staging)?.count(), 0);
    Ok(())
}

const MERGES: &[(&str, u64, &str)] = &[
    (
        "HUDFixes",
        429627,
        "1d9ae3440ec942898d9881c69f9c911879a3cb9ca83346c2d67bf34757bf7a10",
    ),
    (
        "EyeScannerFix",
        89114,
        "a4d02c5ec41ec3d38ac1f937d6d81ba52dd6f0828c94f0f427f0cd26e9e385b4",
    ),
    (
        "ScriptFixes",
        2937,
        "c2021ab1815a7463d5409951d2b2726e10c4e9df61c5455d08bcfb6fd7581dc1",
    ),
    (
        "ModSettingsMenu",
        2815,
        "b161706426b86c2536d006aa88f620d2ed9a87a9ad504c853c8f54ff9d442c99",
    ),
    (
        "PersistentSettings",
        2097,
        "4873b1bd448d24bde75da618d54af22c6ab056adfe623dc8f0cd942ed854d0b3",
    ),
    (
        "FOVCamera",
        20349,
        "9366ebdfd992f9cec1784de4c459406abe0c454f2e1a46fa06445294890c9e09",
    ),
    (
        "ModConvoNodeFix",
        11221,
        "3ac38a8cb325fdf8a3100b1e85527da0cd688f7620f8db96d042fe4ec04feac4",
    ),
    (
        "KarpovPistolFix",
        1693258,
        "65500cea35a30521e2d56418b196464d95e356356ed653ee5a68fe2aa54eb9df",
    ),
    (
        "DebugSaves",
        31935,
        "3e4ada7f1cf49efe5f8ce1f358d7239b8d39a6451961bcd118f1f9b6bcbce991",
    ),
    (
        "SideloaderFramework",
        3535,
        "5e7d8707a1265bfea9169d9c37a8c60a3f7bf5f731fac8e6035e88612b08add0",
    ),
];
