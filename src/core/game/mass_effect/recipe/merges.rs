use sha2::{Digest, Sha256};

use super::super::helper::protocol::{
    Contribution, PlotContribution, ShaderContribution, TargetPackage,
};
use super::super::helper::{FileIdentity, Job};
use super::*;

#[derive(Default)]
pub(super) struct Merges {
    pub(super) originals: BTreeMap<String, FileIdentity>,
    pub(super) generated: BTreeSet<String>,
    tables: Vec<Contribution>,
    table_targets: BTreeSet<String>,
    config: Vec<Contribution>,
    config_targets: BTreeSet<String>,
    plot: Vec<PlotContribution>,
    plot_game: Option<Target>,
    shaders: Vec<ShaderContribution>,
    shader_game: Option<Target>,
    dependencies: BTreeSet<String>,
    pub(super) dlc: Option<super::dlc::Dlc>,
}

impl Merges {
    pub(super) fn originals(originals: BTreeMap<String, helper::FileIdentity>) -> Self {
        Self {
            originals,
            ..Default::default()
        }
    }
    pub(super) fn inspect(
        files: &[PlannedFile],
        packages: &BTreeMap<String, StoredPackage>,
        baseline: &Baseline,
        active_dlc: &BTreeSet<String>,
        control: &Control,
    ) -> Result<Self> {
        let mut result = Self::default();
        let index: BTreeMap<_, _> = files
            .iter()
            .map(|file| (file.destination.relative.to_lowercase(), file))
            .collect();
        let lookup = |path: &str| -> Result<&PlannedFile> {
            index
                .get(&path.to_lowercase())
                .copied()
                .with_context(|| format!("Missing MELE merge input '{path}'"))
        };
        let mut mounts = BTreeMap::new();
        let mut generated = Vec::new();
        for file in files {
            control.check()?;
            let stored = packages
                .get(&file.package)
                .context("Missing MELE merge source")?;
            let plan = &stored.plan;
            let shader = plan.m3gs.iter().find(|plan| {
                plan.source.relative == file.source
                    && file.destination.relative == format!("BioGame/{}", plan.destination)
            });
            let table = plan.m3da.iter().find(|plan| plan.manifest == file.source);
            let config = (plan.manifest.target == Target::Le1)
                .then(|| plan.m3cd.iter().find(|plan| plan.manifest == file.source))
                .flatten();
            let plot = plan
                .plot
                .iter()
                .find(|plan| plan.source.relative == file.source);
            let dlc_merge = plan
                .merge_manifests
                .iter()
                .find(|plan| plan.source.relative == file.source);
            let Some(dlc) = table
                .map(|plan| &plan.dlc)
                .or_else(|| config.map(|plan| &plan.dlc))
                .or_else(|| plot.map(|plan| &plan.dlc))
                .or_else(|| dlc_merge.map(|plan| &plan.dlc))
                .or_else(|| shader.map(|plan| &plan.dlc))
            else {
                continue;
            };
            let mount_path = if plan.manifest.target == Target::Le1 {
                format!("BioGame/DLC/{dlc}/AutoLoad.ini")
            } else {
                format!("BioGame/DLC/{dlc}/CookedPCConsole/Mount.dlc")
            };
            let autoload = lookup(&mount_path)?;
            let owner = packages
                .get(&autoload.package)
                .context("Missing MELE mount source")?;
            let mount_source = SourceFile {
                relative: autoload.source.clone(),
                size: autoload.destination.size,
                sha256: autoload.destination.sha256.clone(),
            };
            let mount = if plan.manifest.target == Target::Le1 {
                let text = super::super::m3da::read_text(&owner.root, &mount_source)?;
                ensure!(
                    format!("{:x}", Sha256::digest(text.as_bytes())) == mount_source.sha256,
                    "MELE mount input changed during recipe inspection"
                );
                super::super::m3da::mount(&text)?
            } else {
                super::super::m3to::mount(&owner.root, &mount_source, plan.manifest.target)?
            };
            ensure!(
                mounts
                    .insert(mount, dlc.to_lowercase())
                    .is_none_or(|previous| previous == dlc.to_lowercase()),
                "Ambiguous MELE DLC mount order; assign distinct mount priorities before rebuilding"
            );
            let manifest = identity(&file.destination)?;
            if let Some(shader) = shader {
                result.shader_game = Some(plan.manifest.target);
                result.shaders.push(ShaderContribution {
                    dlc: dlc.clone(),
                    mount,
                    index: shader.index,
                    shader: manifest,
                });
            } else if let Some(merge) = dlc_merge {
                generated.push((plan.manifest.target, mount, merge.clone()));
            } else if let Some(table) = table {
                let mut inputs = BTreeMap::new();
                for merge in &table.merges {
                    let mapping = plan
                        .files
                        .iter()
                        .find(|mapping| mapping.source == merge.source)
                        .context("Missing M3DA source mapping")?;
                    let input = identity(
                        &lookup(&format!("BioGame/{}", mapping.destination))?.destination,
                    )?;
                    inputs.insert(input.path.clone(), input);
                    let name = super::super::m3da::TARGETS
                        .iter()
                        .find(|name| name.eq_ignore_ascii_case(&merge.target))
                        .context("Unsupported M3DA target")?;
                    result
                        .table_targets
                        .insert(format!("CookedPCConsole/{name}"));
                }
                result.tables.push(Contribution {
                    dlc: dlc.clone(),
                    mount,
                    manifest,
                    packages: inputs.into_values().collect(),
                });
            } else if config.is_some() {
                result.config.push(Contribution {
                    dlc: dlc.clone(),
                    mount,
                    manifest,
                    packages: Vec::new(),
                });
            } else {
                result.plot_game = Some(plan.manifest.target);
                result.plot.push(PlotContribution {
                    dlc: dlc.clone(),
                    mount,
                    manifest,
                });
            }
        }
        if !result.config.is_empty() {
            for file in &baseline.files {
                if let Some(name) = file
                    .relative
                    .strip_prefix("BioGame/CookedPCConsole/Coalesced_")
                    .and_then(|name| name.strip_suffix(".bin"))
                    && name.len() == 3
                    && name.bytes().all(|byte| byte.is_ascii_alphabetic())
                {
                    result.config_targets.insert(identity_baseline(file)?.path);
                }
            }
            ensure!(
                !result.config_targets.is_empty(),
                "M3CD requires baseline Coalesced language files"
            );
        }
        result
            .generated
            .extend(result.table_targets.iter().cloned());
        result
            .generated
            .extend(result.config_targets.iter().cloned());
        if !result.plot.is_empty() {
            result
                .generated
                .insert("CookedPCConsole/PlotManager.pcc".into());
            result.dependencies.extend(
                super::super::helper::jobs::compiler_bases(
                    result.plot_game.context("Missing plot game identity")?,
                )
                .iter()
                .map(|name| format!("CookedPCConsole/{name}")),
            );
        }
        if !result.shaders.is_empty() {
            super::super::m3gs::contributions(&result.shaders)?;
            result.generated.insert(super::super::m3gs::TARGET.into());
        }
        for path in result.generated.iter().chain(&result.dependencies) {
            let original = baseline
                .files
                .iter()
                .find(|file| file.relative == format!("BioGame/{path}"))
                .with_context(|| format!("MELE merge requires baseline file '{path}'"))?;
            result
                .originals
                .insert(path.clone(), identity_baseline(original)?);
        }
        result.dlc = super::dlc::Dlc::inspect(
            generated,
            files,
            baseline,
            active_dlc,
            &mut result.originals,
            control,
        )?;
        if let Some(dlc) = &result.dlc {
            result.generated.extend(
                dlc.outputs()?
                    .into_iter()
                    .filter(|path| !path.starts_with(".merge-ui/")),
            );
        }
        if !result.generated.is_empty() {
            for file in &baseline.files {
                let lower = file.relative.to_lowercase();
                ensure!(
                    ![".m3da", ".m3cd", ".pmu", ".sqm", ".emm", ".m3gs"]
                        .iter()
                        .any(|suffix| lower.ends_with(suffix)),
                    "The restoration baseline contains unmanaged merge contributions; reconcile it before rebuilding"
                );
            }
        }
        result
            .tables
            .sort_by_key(|item| (item.mount, item.manifest.path.to_lowercase()));
        result
            .config
            .sort_by_key(|item| (item.mount, item.manifest.path.to_lowercase()));
        result.shaders.sort_by_key(|item| (item.mount, item.index));
        result.plot.sort_by_key(|item| item.mount);
        let mut current = result.originals.clone();
        for file in files
            .iter()
            .filter(|file| file.destination.relative.starts_with("BioGame/"))
        {
            let input = identity(&file.destination)?;
            current.insert(input.path.clone(), input);
        }
        for phase in 0..5 {
            if let Some(job) = result.job(phase, &current)? {
                job.validate()?;
            }
        }
        Ok(result)
    }

    pub(super) fn job(
        &self,
        phase: usize,
        current: &BTreeMap<String, FileIdentity>,
    ) -> Result<Option<Job>> {
        if phase == 4 {
            return self.dlc.as_ref().map(|dlc| dlc.job(current)).transpose();
        }
        let input = |file: &FileIdentity| -> Result<FileIdentity> {
            current
                .get(&file.path)
                .cloned()
                .with_context(|| format!("Missing MELE candidate contribution '{}'", file.path))
        };
        let contribution = |item: &Contribution| -> Result<Contribution> {
            Ok(Contribution {
                dlc: item.dlc.clone(),
                mount: item.mount,
                manifest: input(&item.manifest)?,
                packages: item.packages.iter().map(&input).collect::<Result<_>>()?,
            })
        };
        let pair = |path: &str| -> Result<TargetPackage> {
            Ok(TargetPackage {
                original: self
                    .originals
                    .get(path)
                    .context("Missing MELE original merge input")?
                    .clone(),
                current: current
                    .get(path)
                    .context("Missing MELE candidate merge input")?
                    .clone(),
            })
        };
        let job = match phase {
            3 if !self.shaders.is_empty() => Job::Shaders {
                game: self.shader_game.context("Missing M3GS game identity")?,
                target: pair(super::super::m3gs::TARGET)?,
                contributions: self
                    .shaders
                    .iter()
                    .map(|item| {
                        Ok(ShaderContribution {
                            dlc: item.dlc.clone(),
                            mount: item.mount,
                            index: item.index,
                            shader: input(&item.shader)?,
                        })
                    })
                    .collect::<Result<_>>()?,
            },
            0 if !self.tables.is_empty() => Job::Tables {
                targets: self
                    .table_targets
                    .iter()
                    .map(|path| pair(path))
                    .collect::<Result<_>>()?,
                contributions: self
                    .tables
                    .iter()
                    .map(&contribution)
                    .collect::<Result<_>>()?,
            },
            1 if !self.config.is_empty() => Job::Config {
                targets: self
                    .config_targets
                    .iter()
                    .map(|path| pair(path))
                    .collect::<Result<_>>()?,
                contributions: self
                    .config
                    .iter()
                    .map(&contribution)
                    .collect::<Result<_>>()?,
            },
            2 if !self.plot.is_empty() => Job::Plot {
                game: self.plot_game.context("Missing plot game identity")?,
                target: pair("CookedPCConsole/PlotManager.pcc")?,
                dependencies: self
                    .dependencies
                    .iter()
                    .map(|path| {
                        current
                            .get(path)
                            .cloned()
                            .context("Missing plot compiler dependency")
                    })
                    .collect::<Result<_>>()?,
                contributions: self
                    .plot
                    .iter()
                    .map(|item| {
                        Ok(PlotContribution {
                            dlc: item.dlc.clone(),
                            mount: item.mount,
                            manifest: input(&item.manifest)?,
                        })
                    })
                    .collect::<Result<_>>()?,
            },
            _ => return Ok(None),
        };
        Ok(Some(job))
    }
}

pub(super) fn identity(file: &SourceFile) -> Result<FileIdentity> {
    Ok(FileIdentity {
        path: file
            .relative
            .strip_prefix("BioGame/")
            .context("MELE merge input is outside BioGame")?
            .into(),
        size: file.size,
        sha256: file.sha256.clone(),
    })
}

fn identity_baseline(file: &super::super::baseline::BaselineFile) -> Result<FileIdentity> {
    identity(&SourceFile {
        relative: file.relative.clone(),
        size: file.size,
        sha256: file.sha256.clone(),
    })
}
