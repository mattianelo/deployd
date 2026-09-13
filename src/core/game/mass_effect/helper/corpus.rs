use super::super::{Target, m3za, package::SourceFile};
use super::*;
use protocol::{PlotContribution, TargetPackage, TlkChange};

const GAME: &str = "docs/modTesting/Mass Effect Legendary Edition (Game)/Game/ME1";
const MOD: &str = "docs/modTesting/LE1 Community Patch";
const CODEC: (&str, u64, &str) = (
    "Binaries/Win64/oo2core_8_win64.dll",
    1007616,
    "d42940381611cda3b8555f6eb9fcb1bc3b1a3b96d7e24cb98738f4b71653d415",
);
const PLOT: (&str, u64, &str) = (
    "CookedPCConsole/PlotManager.pcc",
    48632,
    "2857097a8325760aa7f9a8e2edbcc9066676e8626525360936d36a88fba63d06",
);
const BASES: &[(&str, u64, &str)] = &[
    (
        "CookedPCConsole/Core.pcc",
        26841,
        "fd74ff917c2fce6628c872d8eeb5a8204372f4709e56e71b09ff1fb20266ffea",
    ),
    (
        "CookedPCConsole/Engine.pcc",
        9843689,
        "296308fba3fe6294f8ed4eba5796fcc54e75e51837ff9cc261e1151124f4ad13",
    ),
    (
        "CookedPCConsole/GFxUI.pcc",
        13553,
        "dd58ce9c2548da72e4351e682af0971af634dd61f15201458af219c32f7fd2ff",
    ),
    (
        "CookedPCConsole/PlotManagerMap.pcc",
        2326,
        "a14c5b5cf847d61144d1b35a8700e8666c48c3c6bc763e80f618fbf89248538d",
    ),
    (
        "CookedPCConsole/SFXOnlineFoundation.pcc",
        37354,
        "cdb2147d824c73e7091600b1dc1e5693c53ce7edef5249bf119e88ab23e69ca0",
    ),
    (
        "CookedPCConsole/SFXGame.pcc",
        6018634,
        "79a1a3cc7afe2714d3d297dcb737f26314e7a302ffb854b7534a433d262ae9a5",
    ),
    (
        "CookedPCConsole/SFXStrategicAI.pcc",
        54161,
        "ad034539f45f91a6d0eaa2951ab3b0d6898f9f41b29bd506fa15ced06d4e6e13",
    ),
    (
        "CookedPCConsole/SFXGameContent_Powers.pcc",
        40673,
        "4c67392c31946a895e86f9764cd80abead2a24497dbcdc892a715dccab5e4b39",
    ),
];

struct Corpus {
    _root: TempDir,
    backend: Backend,
    inputs: Inputs,
    staging: PathBuf,
    sources: Vec<(PathBuf, FileIdentity)>,
}

impl Corpus {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir_in("/build/mele")?;
        for directory in ["game", "input", "original", "staging"] {
            fs::create_dir(root.path().join(directory))?;
        }
        let configuration: serde_json::Value =
            serde_json::from_str(include_str!("../../../../../helpers/mele/toolchain.json"))?;
        let sdk = configuration["sdk"]["version"]
            .as_str()
            .context("Missing SDK version")?;
        let backend = Backend {
            runtime: PathBuf::from(format!("/build/mele/sdk-{sdk}/dotnet")),
            assembly: PathBuf::from(
                "/build/mele/source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll",
            ),
            native_library: PathBuf::from(
                "/build/mele/source/Deployd.Mele/bin/Release/net10.0/libdeployd_oodle.so",
            ),
        };
        backend.validate()?;
        let mut corpus = Self {
            backend,
            inputs: Inputs {
                game: root.path().join("game"),
                candidate: root.path().join("input"),
                original: Some(root.path().join("original")),
            },
            staging: root.path().join("staging"),
            sources: Vec::new(),
            _root: root,
        };
        corpus.copy(Path::new(GAME), CODEC, &corpus.inputs.game.clone(), CODEC.0)?;
        Ok(corpus)
    }

    fn copy(
        &mut self,
        root: &Path,
        specification: (&str, u64, &str),
        destination: &Path,
        relative: &str,
    ) -> Result<FileIdentity> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(root);
        let source = FileIdentity {
            path: specification.0.into(),
            size: specification.1,
            sha256: specification.2.into(),
        };
        files::identity(&root, &source, &tests::control())?;
        self.sources.push((root.clone(), source.clone()));
        let output = destination.join(relative);
        fs::create_dir_all(output.parent().context("Missing copy parent")?)?;
        fs::copy(root.join(&source.path), output)?;
        Ok(FileIdentity {
            path: relative.into(),
            ..source
        })
    }

    fn verify(&self) -> Result<()> {
        for (root, source) in &self.sources {
            files::identity(root, source, &tests::control())?;
        }
        Ok(())
    }

    async fn run(&self, job: Job) -> Result<ValidatedOutput> {
        transform(
            self.backend.clone(),
            self.inputs.clone(),
            self.staging.clone(),
            job,
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_, _| {}),
        )
        .await
    }
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper and the maintainer-supplied disposable corpus"]
async fn supervises_real_community_patch_tlk_without_deploying() -> Result<()> {
    let mut corpus = Corpus::new()?;
    corpus.inputs.original = None;
    let package = (
        "CookedPCConsole/BIOA_END20_Bridge_CIN_LOC_INT.pcc",
        174842,
        "ee26f3c1debb940b01730a034fd156e6f57871572f34846bbb7700100ae1aa3a",
    );
    let target = corpus.copy(
        &Path::new(GAME).join("BioGame"),
        package,
        &corpus.inputs.candidate.clone(),
        package.0,
    )?;
    let source = SourceFile {
        relative: "GAME1_EMBEDDED_TLK/CombinedTLKMergeData.m3za".into(),
        size: 85371,
        sha256: "b9ea76366e24f33166871a059284974a32e0ef3fb1c7ad97b41d40ea3fa58619".into(),
    };
    let mod_root = Path::new(env!("CARGO_MANIFEST_DIR")).join(MOD);
    let plan = m3za::inspect(&mod_root, &source, Target::Le1)?;
    let changes: Vec<_> = plan
        .updates
        .into_iter()
        .filter(|update| {
            update
                .package
                .eq_ignore_ascii_case("BIOA_END20_Bridge_CIN_LOC_INT.pcc")
        })
        .map(|update| TlkChange {
            target: target.path.clone(),
            export: update.export,
            strings: update.strings,
        })
        .collect();
    ensure!(!changes.is_empty(), "Missing frozen TLK changes");
    let job = Job::Tlk {
        targets: vec![target.clone()],
        changes,
    };
    let result = corpus.run(job).await;
    corpus.verify()?;
    m3za::inspect(&mod_root, &source, Target::Le1)?;
    let output = result?;
    assert_eq!(output.files.len(), 1);
    assert_ne!(output.files[0].sha256, target.sha256);
    drop(output);
    assert_eq!(fs::read_dir(&corpus.staging)?.count(), 0);
    Ok(())
}

// @variants: both
#[tokio::test]
#[ignore = "Requires the pinned Linux helper and the maintainer-supplied disposable corpus"]
async fn supervises_real_community_patch_plot_and_original_restoration() -> Result<()> {
    let mut corpus = Corpus::new()?;
    let root = Path::new(GAME).join("BioGame");
    let original = corpus.copy(
        &root,
        PLOT,
        &corpus
            .inputs
            .original
            .clone()
            .context("Missing originals")?,
        PLOT.0,
    )?;
    let current = corpus.copy(&root, PLOT, &corpus.inputs.candidate.clone(), PLOT.0)?;
    let target = TargetPackage { original, current };
    let mut dependencies = Vec::new();
    for specification in BASES {
        dependencies.push(corpus.copy(
            &root,
            *specification,
            &corpus.inputs.candidate.clone(),
            specification.0,
        )?);
    }
    let manifest = corpus.copy(
        Path::new(MOD),
        (
            "DLC_MOD_LE1CP/CookedPCConsole/PlotManagerUpdate.pmu",
            3017,
            "bf2276c59d680cafe1a4fe8741e62be69d91cfe54629abd13075355c267a524f",
        ),
        &corpus.inputs.candidate.clone(),
        "DLC/DLC_MOD_LE1CP/CookedPCConsole/PlotManagerUpdate.pmu",
    )?;
    let job = Job::Plot {
        game: super::super::Target::Le1,
        target: target.clone(),
        dependencies,
        contributions: vec![PlotContribution {
            dlc: "DLC_MOD_LE1CP".into(),
            mount: 5,
            manifest,
        }],
    };
    let result = corpus.run(job).await;
    corpus.verify()?;
    let output = result?;
    assert_ne!(output.files[0].sha256, target.original.sha256);
    let mut removal_target = target;
    fs::copy(
        output.root().join(PLOT.0),
        corpus.inputs.candidate.join(PLOT.0),
    )?;
    removal_target.current = output.files[0].clone();
    let removed = corpus
        .run(Job::Plot {
            game: super::super::Target::Le1,
            target: removal_target,
            dependencies: Vec::new(),
            contributions: Vec::new(),
        })
        .await;
    corpus.verify()?;
    let removed = removed?;
    assert_eq!(removed.files[0].sha256, PLOT.2);
    drop((output, removed));
    assert_eq!(fs::read_dir(&corpus.staging)?.count(), 0);
    Ok(())
}

mod merges;
