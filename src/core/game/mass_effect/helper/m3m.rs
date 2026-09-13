use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::super::m3m::{self, M3mPlan, Operation, Script};
use super::protocol::{FileIdentity, Job, M3mKind, M3mOperation, TargetPackage};
use super::{Abandon, Control, files, jobs, overlaps};

pub(crate) struct Sources {
    pub(crate) package: PathBuf,
    pub(crate) original: PathBuf,
    pub(crate) candidate: PathBuf,
    pub(crate) packages: Vec<TargetPackage>,
    pub(crate) plans: Vec<M3mPlan>,
}

pub(crate) struct Prepared {
    directory: TempDir,
    pub(crate) job: Job,
}

impl Prepared {
    pub(crate) fn root(&self) -> &Path {
        self.directory.path()
    }
}

pub(crate) async fn prepare(
    sources: Sources,
    parent: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<Prepared> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandon = Abandon(abandoned.clone());
    let control = Control {
        cancelled,
        abandoned,
    };
    let location = tokio::select! {
        biased;
        _ = control.stopped() => anyhow::bail!("M3M preparation cancelled"),
        location = crate::core::location_recovery::activity_lock().read_owned() => location,
    };
    let worker_control = control.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _location = location;
        stage(sources, &parent, &worker_control)
    })
    .await
    .context("M3M preparation worker failed")?;
    control.check()?;
    result
}

pub(in crate::core::game::mass_effect) async fn prepare_with_lease(
    sources: Sources,
    parent: PathBuf,
    control: Control,
    lease: Arc<super::Lease>,
) -> Result<Prepared> {
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        stage(sources, &parent, &control)
    })
    .await
    .context("M3M preparation worker failed")?
}

fn stage(sources: Sources, parent: &Path, control: &Control) -> Result<Prepared> {
    control.check()?;
    files::directory(parent)?;
    for root in [&sources.package, &sources.original, &sources.candidate] {
        files::directory(root)?;
        ensure!(
            !overlaps(root, parent),
            "M3M preparation must be separate from every source folder"
        );
    }
    ensure!(
        !sources.plans.is_empty()
            && sources.plans.len() <= 1024
            && sources.packages.len() <= m3m::TARGETS.len(),
        "Invalid M3M preparation inputs"
    );
    let game = sources.plans.first().context("Missing M3M plan")?.game;
    ensure!(
        sources.plans.iter().all(|plan| plan.game == game),
        "M3M preparation mixes game targets"
    );
    let mut source_bytes = 0_u64;
    for plan in &sources.plans {
        source_bytes = source_bytes
            .checked_add(plan.source.size)
            .context("M3M source size overflow")?;
        ensure!(
            source_bytes <= 4 * 1024 * 1024 * 1024,
            "M3M sources exceed the preparation size limit"
        );
    }
    let mut available = BTreeMap::new();
    for target in &sources.packages {
        ensure!(
            target.original.path == target.current.path
                && target
                    .current
                    .path
                    .strip_prefix("CookedPCConsole/")
                    .is_some_and(|name| m3m::TARGETS.contains(&name)
                        || jobs::compiler_bases(game).contains(&name))
                && available
                    .insert(target.current.path.clone(), target)
                    .is_none(),
            "Invalid or duplicate available M3M package"
        );
    }
    let directory = tempfile::Builder::new()
        .prefix("mele-m3m-")
        .tempdir_in(parent)?;
    let mut builder = Builder {
        root: directory.path(),
        assets: BTreeMap::new(),
        scripts: BTreeMap::new(),
        jobs: Vec::new(),
        bytes: 0,
        control,
    };
    for (index, plan) in sources.plans.iter().enumerate() {
        control.check()?;
        files::identity(
            &sources.package,
            &FileIdentity {
                path: plan.source.relative.clone(),
                size: plan.source.size,
                sha256: plan.source.sha256.clone(),
            },
            control,
        )?;
        let embedded = m3m::materialize(&sources.package, plan)?;
        for file in &plan.files {
            let targets: Vec<_> = file
                .target_candidates
                .iter()
                .map(|name| format!("CookedPCConsole/{name}"))
                .filter(|path| available.contains_key(path))
                .collect();
            ensure!(
                !targets.is_empty(),
                "M3M target '{}' is unavailable in the candidate installation",
                file.target
            );
            for target in targets {
                for change in &file.changes {
                    for operation in &change.operations {
                        builder.operation(index, &target, &change.entry, operation, &embedded)?;
                    }
                }
            }
        }
    }
    let names: BTreeSet<_> = builder.jobs.iter().map(|job| job.target.clone()).collect();
    let required = jobs::compiler_dependencies(game, &builder.jobs, &names)?;
    let mut targets = Vec::new();
    let mut dependencies = Vec::new();
    for name in names.iter().chain(required.iter()) {
        let pair = available
            .get(name)
            .context("A verified M3M compiler dependency is missing")?;
        copy(&sources.candidate, &pair.current, directory.path(), control)?;
        builder.bytes = builder
            .bytes
            .checked_add(pair.current.size)
            .context("M3M input size overflow")?;
        if names.contains(name) {
            files::identity(&sources.original, &pair.original, control)?;
            builder.bytes = builder
                .bytes
                .checked_add(pair.original.size)
                .context("M3M input size overflow")?;
            targets.push((*pair).clone());
        } else {
            dependencies.push(pair.current.clone());
        }
        ensure!(
            builder.bytes <= 4 * 1024 * 1024 * 1024,
            "M3M preparation exceeds the job size limit"
        );
    }
    let job = Job::M3m {
        game,
        targets,
        dependencies,
        assets: builder.assets.into_values().collect(),
        scripts: builder.scripts.into_values().collect(),
        jobs: builder.jobs,
    };
    job.validate()?;
    control.check()?;
    Ok(Prepared { directory, job })
}

struct Builder<'a> {
    root: &'a Path,
    assets: BTreeMap<String, FileIdentity>,
    scripts: BTreeMap<String, FileIdentity>,
    jobs: Vec<M3mOperation>,
    bytes: u64,
    control: &'a Control,
}

impl Builder<'_> {
    fn operation(
        &mut self,
        index: usize,
        target: &str,
        entry: &str,
        operation: &Operation,
        embedded: &BTreeMap<String, Vec<u8>>,
    ) -> Result<()> {
        let mut add_script = |kind, script: &Script| -> Result<()> {
            let name = match script {
                Script::Inline { name, .. } | Script::Asset { name } => name,
            };
            let text = match script {
                Script::Inline { text, .. } => text.as_str(),
                Script::Asset { name } => {
                    std::str::from_utf8(embedded.get(name).context("Missing verified M3M script")?)?
                        .trim_start_matches('\u{feff}')
                }
            };
            let path = self.store(index, name, text.as_bytes(), false)?;
            self.push(M3mOperation {
                target: target.into(),
                entry: entry.into(),
                kind,
                input: path,
                source_entry: String::new(),
                allow_new: false,
            })
        };
        match operation {
            Operation::Asset {
                asset,
                entry: source_entry,
                allow_new,
            } => {
                let path = self.store(
                    index,
                    asset,
                    embedded.get(asset).context("Missing verified M3M asset")?,
                    true,
                )?;
                self.push(M3mOperation {
                    target: target.into(),
                    entry: entry.into(),
                    kind: M3mKind::Asset,
                    input: path,
                    source_entry: source_entry.clone(),
                    allow_new: *allow_new,
                })?;
            }
            Operation::Class { script } => add_script(M3mKind::Class, script)?,
            Operation::Function { script } => add_script(M3mKind::Function, script)?,
            Operation::Members { scripts } => {
                for script in scripts {
                    add_script(M3mKind::Member, script)?;
                }
            }
        }
        Ok(())
    }

    fn push(&mut self, job: M3mOperation) -> Result<()> {
        self.control.check()?;
        ensure!(
            self.jobs.len() < 4096,
            "M3M localization expands beyond the job count limit"
        );
        self.jobs.push(job);
        Ok(())
    }

    fn store(&mut self, index: usize, name: &str, bytes: &[u8], asset: bool) -> Result<String> {
        self.control.check()?;
        ensure!(
            !bytes.is_empty()
                && bytes.len()
                    <= if asset {
                        128 * 1024 * 1024
                    } else {
                        4 * 1024 * 1024
                    },
            "Invalid prepared M3M input size"
        );
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        let prefix = if asset { "Assets" } else { "Scripts" };
        // Unreal import resolution uses the source package filename.
        let path = format!("{prefix}/{index}/{sha256}/{name}");
        files::relative(&path)?;
        let entries = if asset {
            &mut self.assets
        } else {
            &mut self.scripts
        };
        if !entries.contains_key(&path) {
            ensure!(
                entries.len() < if asset { 1024 } else { 4096 },
                "Too many prepared M3M inputs"
            );
            self.bytes += bytes.len() as u64;
            ensure!(
                self.bytes <= 4 * 1024 * 1024 * 1024,
                "M3M preparation exceeds the job size limit"
            );
            let destination = self.root.join(&path);
            fs::create_dir_all(destination.parent().context("Missing M3M input parent")?)?;
            File::create_new(destination)?.write_all(bytes)?;
            entries.insert(
                path.clone(),
                FileIdentity {
                    path: path.clone(),
                    size: bytes.len() as u64,
                    sha256,
                },
            );
        }
        Ok(path)
    }
}

fn copy(root: &Path, input: &FileIdentity, destination: &Path, control: &Control) -> Result<()> {
    files::identity(root, input, control)?;
    let path = destination.join(&input.path);
    fs::create_dir_all(path.parent().context("Missing M3M input parent")?)?;
    let mut output = File::create_new(path)?;
    let mut source = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join(&input.path))?;
    let metadata = source.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1 && metadata.len() == input.size,
        "M3M candidate changed before copying"
    );
    let mut buffer = [0_u8; 65536];
    let mut remaining = input.size;
    while remaining > 0 {
        control.check()?;
        let count = source.read(&mut buffer[..remaining.min(65536) as usize])?;
        ensure!(count > 0, "M3M candidate changed during preparation");
        output.write_all(&buffer[..count])?;
        remaining -= count as u64;
    }
    ensure!(
        source.read(&mut buffer[..1])? == 0,
        "M3M candidate grew during preparation"
    );
    files::identity(destination, input, control)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::super::{Target, package::SourceFile};
    use super::super::tests::control;
    use super::*;

    fn fixture() -> Result<(TempDir, Sources, PathBuf)> {
        let root = tempfile::tempdir()?;
        for directory in [
            "package",
            "original/CookedPCConsole",
            "candidate/CookedPCConsole",
            "preparation",
        ] {
            fs::create_dir_all(root.path().join(directory))?;
        }
        let mut sources = Sources {
            package: root.path().join("package"),
            original: root.path().join("original"),
            candidate: root.path().join("candidate"),
            packages: Vec::new(),
            plans: Vec::new(),
        };
        for name in [
            "Core.pcc",
            "Engine.pcc",
            "Startup_DE.pcc",
            "Startup_INT.pcc",
        ] {
            let identity = FileIdentity {
                path: format!("CookedPCConsole/{name}"),
                size: 4,
                sha256: format!("{:x}", Sha256::digest([0xc1, 0x83, 0x2a, 0x9e])),
            };
            fs::write(
                sources.original.join(&identity.path),
                [0xc1, 0x83, 0x2a, 0x9e],
            )?;
            fs::write(
                sources.candidate.join(&identity.path),
                [0xc1, 0x83, 0x2a, 0x9e],
            )?;
            sources.packages.push(TargetPackage {
                original: identity.clone(),
                current: identity,
            });
        }
        let manifest = json!({"game":"LE1", "files":[
            {"filename":"Engine.pcc", "changes":[{"entryname":"Example.Fn", "scriptupdate":{"scriptfilename":"Example.uc", "scripttext":"function Fn() {}"}}]},
            {"filename":"Startup_INT.pcc", "applytoalllocalizations":true, "changes":[{"entryname":"Example.Asset", "assetupdate":{"assetname":"Example.pcc", "entryname":"Source.Asset", "canmergeasnew":false}}]}
        ]}).to_string();
        let mut bytes = b"M3MM\x01".to_vec();
        bytes.extend_from_slice(&(manifest.len() as i32 + 1).to_le_bytes());
        bytes.extend_from_slice(manifest.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&1_i32.to_le_bytes());
        bytes.extend_from_slice(b"MMV1");
        bytes.extend_from_slice(&12_i32.to_le_bytes());
        bytes.extend_from_slice(b"Example.pcc\0");
        bytes.extend_from_slice(&4_i32.to_le_bytes());
        bytes.extend_from_slice(&[0xc1, 0x83, 0x2a, 0x9e]);
        let source = SourceFile {
            relative: "Example.m3m".into(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
        };
        fs::write(sources.package.join(&source.relative), bytes)?;
        sources
            .plans
            .push(m3m::inspect(&sources.package, &source, Target::Le1)?);
        let parent = root.path().join("preparation");
        Ok((root, sources, parent))
    }

    // @variants: both
    #[test]
    fn preserves_order_and_expands_only_available_localizations() -> Result<()> {
        let (_root, mut sources, parent) = fixture()?;
        sources.plans.push(sources.plans[0].clone());
        let output = stage(sources, &parent, &control())?;
        let Job::M3m {
            targets,
            dependencies,
            assets,
            scripts,
            jobs,
            ..
        } = &output.job
        else {
            panic!("Expected M3M");
        };
        assert_eq!(
            jobs.iter()
                .map(|job| job.target.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CookedPCConsole/Engine.pcc",
                "CookedPCConsole/Startup_DE.pcc",
                "CookedPCConsole/Startup_INT.pcc",
                "CookedPCConsole/Engine.pcc",
                "CookedPCConsole/Startup_DE.pcc",
                "CookedPCConsole/Startup_INT.pcc"
            ]
        );
        assert_eq!(
            (
                targets.len(),
                dependencies.len(),
                assets.len(),
                scripts.len()
            ),
            (3, 1, 2, 2)
        );
        assert_eq!(dependencies[0].path, "CookedPCConsole/Core.pcc");
        assert!(
            assets
                .iter()
                .all(|asset| asset.path.ends_with("/Example.pcc"))
        );
        assert!(
            scripts
                .iter()
                .all(|script| script.path.ends_with("/Example.uc"))
        );
        for input in output.job.inputs() {
            files::identity(output.root(), input, &control())?;
        }
        let path = output.root().to_path_buf();
        drop(output);
        assert!(!path.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_missing_dependencies_changed_plans_and_cancellation() -> Result<()> {
        for mode in 0..4 {
            let (_root, mut sources, parent) = fixture()?;
            let control = control();
            match mode {
                0 => sources
                    .packages
                    .retain(|target| target.current.path != "CookedPCConsole/Core.pcc"),
                1 => sources.plans[0].files[0].changes[0].entry = "Tampered.Fn".into(),
                2 => control
                    .cancelled
                    .store(true, std::sync::atomic::Ordering::SeqCst),
                _ => sources
                    .packages
                    .retain(|target| !target.current.path.contains("Startup_")),
            }
            assert!(stage(sources, &parent, &control).is_err());
            assert_eq!(fs::read_dir(&parent)?.count(), 0);
        }
        Ok(())
    }
    // @variants: both
    #[tokio::test]
    async fn cancels_preparation_while_waiting_for_folder_access() -> Result<()> {
        let (_root, sources, parent) = fixture()?;
        let location = crate::core::location_recovery::activity_lock()
            .write_owned()
            .await;
        let cancelled = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(prepare(sources, parent.clone(), cancelled.clone()));
        tokio::task::yield_now().await;
        cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), task)
                .await??
                .is_err()
        );
        assert_eq!(fs::read_dir(&parent)?.count(), 0);
        drop(location);
        Ok(())
    }
}
