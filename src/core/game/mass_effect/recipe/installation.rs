use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use super::super::baseline::Baseline;
use super::super::helper::{FileIdentity, protocol::TlkChange};
use super::super::m3cd::M3cdPlan;
use super::super::m3m::{M3mPlan, Operation};
use super::super::m3to::M3toPlan;
use super::super::package::SourceFile;
use super::{Control, PlannedFile, Recipe, StoredPackage, Target};

#[derive(Clone)]
pub(super) struct Step {
    pub(super) removed: Vec<String>,
    pub(super) package: String,
    pub(super) files: Vec<PlannedFile>,
    pub(super) m3m: Vec<M3mPlan>,
    pub(super) m3m_inputs: BTreeSet<String>,
    pub(super) tlk: Vec<TlkChange>,
    pub(super) dlc_config: Vec<M3cdPlan>,
    pub(super) m3to: Vec<M3toPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkippedTlk {
    pub(crate) package: String,
    pub(crate) target: String,
    pub(crate) export: String,
}

#[derive(Default)]
pub(super) struct Installation {
    pub(super) steps_prepared: bool,
    pub(super) defer_textures: bool,
    pub(super) removals: super::super::removal::Removals,
    pub(super) dlc: BTreeSet<String>,
    pub(super) versions: BTreeMap<String, String>,
    pub(super) options: BTreeMap<String, BTreeSet<String>>,
    pub(super) steps: Vec<Step>,
    pub(super) originals: BTreeMap<String, FileIdentity>,
    pub(super) generated: BTreeSet<String>,
    pub(super) skipped: Vec<SkippedTlk>,
}

impl Installation {
    pub(super) fn has_raw_m3to(&self) -> bool {
        self.steps
            .iter()
            .flat_map(|step| &step.m3to)
            .any(|plan| !plan.is_compiled())
    }

    pub(super) fn inspect(
        recipe: &Recipe,
        packages: &mut BTreeMap<String, StoredPackage>,
        baseline: &Baseline,
        control: &Control,
        mut preparation: Option<&mut super::preparation::Session>,
    ) -> Result<Self> {
        let mut result = Self::default();
        let mut available: BTreeSet<_> = baseline
            .files
            .iter()
            .map(|file| file.relative.clone())
            .collect();
        let mut sizes: BTreeMap<_, _> = baseline
            .files
            .iter()
            .map(|file| (file.relative.to_lowercase(), Some(file.size)))
            .collect();
        let mut effective = BTreeMap::new();
        for selection in recipe.packages.iter().filter(|selection| selection.enabled) {
            control.check()?;
            let stored = packages
                .get_mut(&selection.id)
                .context("Missing MELE installation source")?;
            let dlc: BTreeSet<String> = available
                .iter()
                .filter_map(|path| super::super::removal::dlc(path))
                .map(str::to_lowercase)
                .collect();
            if let Some(session) = preparation.as_deref_mut() {
                session.verify_conditions(&stored.plan)?;
            }
            if let Some(target) = &stored.plan.manifest.localization_target {
                ensure!(
                    dlc.contains(&target.to_lowercase()),
                    "Install the DLC '{target}' before its localization package"
                );
            }
            stored.resolve(
                &selection.options,
                &super::super::alternates::Context {
                    available: &dlc,
                    sizes: &sizes,
                    versions: Some(&result.versions),
                    options: Some(&result.options),
                },
            )?;
            let stored = packages
                .get(&selection.id)
                .context("Missing resolved MELE source")?;
            let mut step = Step {
                removed: Vec::new(),
                package: selection.id.clone(),
                files: Vec::new(),
                m3m: stored.plan.m3m.clone(),
                m3m_inputs: BTreeSet::new(),
                tlk: Vec::new(),
                dlc_config: if recipe.target == Target::Le2 {
                    stored.plan.m3cd.clone()
                } else {
                    Vec::new()
                },
                m3to: stored.plan.m3to.clone(),
            };
            for mapping in &stored.plan.files {
                let source = stored
                    .plan
                    .sources
                    .iter()
                    .find(|file| file.relative == mapping.source)
                    .context("Missing MELE installation file")?;
                let file = PlannedFile {
                    package: selection.id.clone(),
                    source: mapping.source.clone(),
                    destination: SourceFile {
                        relative: super::super::binary::game_path(&mapping.destination)?,
                        size: source.size,
                        sha256: source.sha256.clone(),
                    },
                };
                available.insert(file.destination.relative.clone());
                result.removals.paths.remove(&file.destination.relative);
                effective.insert(file.destination.relative.to_lowercase(), file.clone());
                step.files.push(file);
            }
            for (_, dlc) in &stored.plan.manifest.dlc {
                result
                    .options
                    .insert(dlc.to_lowercase(), stored.plan.active_options.clone());
                result
                    .versions
                    .insert(dlc.to_lowercase(), stored.plan.manifest.version.clone());
            }
            if !step.m3m.is_empty() || stored.plan.embedded_tlk.is_some() {
                dependencies(
                    &stored.plan.manifest.required_dlc,
                    &stored.plan.manifest.format,
                    &available,
                    &result.versions,
                    &result.options,
                )?;
            }
            for plan in &step.m3m {
                for file in &plan.files {
                    let targets: Vec<_> = file
                        .target_candidates
                        .iter()
                        .map(|name| format!("CookedPCConsole/{name}"))
                        .filter(|path| available.contains(&format!("BioGame/{path}")))
                        .collect();
                    ensure!(
                        !targets.is_empty(),
                        "M3M target '{}' is unavailable in this installation step",
                        file.target
                    );
                    for target in targets {
                        step.m3m_inputs.insert(target.clone());
                        result.generated.insert(target.clone());
                        if file
                            .changes
                            .iter()
                            .flat_map(|change| &change.operations)
                            .any(|operation| !matches!(operation, Operation::Asset { .. }))
                        {
                            step.m3m_inputs
                                .extend(super::super::helper::jobs::compiler_inputs(
                                    plan.game, &target,
                                )?);
                        }
                    }
                }
            }
            for path in &step.m3m_inputs {
                result.original(baseline, path)?;
            }
            if let Some(tlk) = &stored.plan.embedded_tlk {
                for update in &tlk.updates {
                    if stored.plan.manifest.format.starts_with('9')
                        && update.option_key.as_ref().is_some_and(|key| {
                            !selection.options.contains(key)
                                && !stored.plan.active_tlk.contains(key)
                        })
                    {
                        continue;
                    }
                    if update.strings.is_empty() {
                        continue;
                    }
                    let Some(path) =
                        resolve_tlk(&update.package, &available, &effective, packages)?
                    else {
                        result.skipped.push(SkippedTlk {
                            package: selection.id.clone(),
                            target: update.package.clone(),
                            export: update.export.clone(),
                        });
                        continue;
                    };
                    super::super::journal::destination(&format!("BioGame/{path}"))?;
                    if baseline
                        .files
                        .iter()
                        .any(|file| file.relative == format!("BioGame/{path}"))
                    {
                        result.original(baseline, &path)?;
                    }
                    result.generated.insert(path.clone());
                    step.tlk.push(TlkChange {
                        target: path,
                        export: update.export.clone(),
                        strings: update.strings.clone(),
                    });
                }
            }
            step.removed = result.removals.retire(
                &stored.plan.manifest.obsolete_dlc,
                &mut available,
                control,
            )?;
            for dlc in &stored.plan.manifest.obsolete_dlc {
                result.versions.remove(&dlc.to_lowercase());
                result.options.remove(&dlc.to_lowercase());
            }
            for path in &step.removed {
                effective.remove(&path.to_lowercase());
                if let Some(path) = path.strip_prefix("BioGame/") {
                    result.generated.remove(path);
                }
            }
            for file in &step.files {
                sizes.insert(
                    file.destination.relative.to_lowercase(),
                    Some(file.destination.size),
                );
            }
            for path in step
                .m3m
                .iter()
                .flat_map(|plan| &plan.files)
                .flat_map(|file| &file.target_candidates)
                .map(|name| format!("CookedPCConsole/{name}"))
                .chain(step.tlk.iter().map(|tlk| tlk.target.clone()))
            {
                let relative = format!("BioGame/{path}");
                if available.contains(&relative) {
                    sizes.insert(relative.to_lowercase(), None);
                }
            }
            for path in &step.removed {
                sizes.remove(&path.to_lowercase());
            }
            if let Some(session) = preparation.as_deref_mut() {
                for file in session.step(&step, &result, packages)? {
                    sizes.insert(file.relative.to_lowercase(), Some(file.size));
                }
            }
            result.steps.push(step);
        }
        let mut texture_layouts = BTreeMap::new();
        for plan in result.steps.iter().flat_map(|step| &step.m3to) {
            let compiled = plan.is_compiled();
            if let Some(previous) = texture_layouts.insert(plan.dlc().to_lowercase(), compiled) {
                ensure!(
                    previous == compiled,
                    "Raw and precompiled M3TO packages cannot target the same DLC in one installation"
                );
            }
        }
        result.removals.validate(recipe.target)?;
        result.dlc = available
            .iter()
            .filter_map(|path| super::super::removal::dlc(path))
            .map(str::to_lowercase)
            .collect();
        control.check()?;
        Ok(result)
    }

    fn original(&mut self, baseline: &Baseline, path: &str) -> Result<()> {
        let file = baseline
            .files
            .iter()
            .find(|file| file.relative == format!("BioGame/{path}"))
            .with_context(|| format!("MELE installation requires baseline input '{path}'"))?;
        self.originals.insert(
            path.into(),
            FileIdentity {
                path: path.into(),
                size: file.size,
                sha256: file.sha256.clone(),
            },
        );
        Ok(())
    }
}

fn resolve_tlk(
    name: &str,
    available: &BTreeSet<String>,
    effective: &BTreeMap<String, PlannedFile>,
    packages: &BTreeMap<String, StoredPackage>,
) -> Result<Option<String>> {
    let mut matches = BTreeMap::new();
    for path in available.iter().filter(|path| {
        path.starts_with("BioGame/")
            && path
                .rsplit('/')
                .next()
                .is_some_and(|file| file.eq_ignore_ascii_case(name))
    }) {
        let relative = path
            .strip_prefix("BioGame/")
            .context("TLK target is outside BioGame")?;
        let parts: Vec<_> = relative.split('/').collect();
        let rank = if parts.len() >= 2 && parts[0] == "CookedPCConsole" {
            None
        } else {
            ensure!(
                parts.len() >= 4 && parts[0] == "DLC" && parts[2] == "CookedPCConsole",
                "TLK target has an unsupported package layout"
            );
            let autoload = effective.get(&format!("BioGame/DLC/{}/AutoLoad.ini", parts[1]).to_lowercase())
                .context("TLK priority requires managed DLC mount metadata; reconcile the installation before rebuilding")?;
            let owner = packages
                .get(&autoload.package)
                .context("Missing TLK mount source")?;
            let text = super::super::m3da::read_text(
                &owner.root,
                &SourceFile {
                    relative: autoload.source.clone(),
                    size: autoload.destination.size,
                    sha256: autoload.destination.sha256.clone(),
                },
            )?;
            ensure!(
                format!("{:x}", Sha256::digest(text.as_bytes())) == autoload.destination.sha256,
                "TLK mount input changed during inspection"
            );
            Some(super::super::m3da::mount(&text)?)
        };
        ensure!(
            matches.insert(rank, relative.to_owned()).is_none(),
            "Ambiguous TLK package mount precedence for '{name}'"
        );
    }
    Ok(matches.pop_last().map(|(_, path)| path))
}

fn dependencies(
    required: &[String],
    format: &str,
    available: &BTreeSet<String>,
    versions: &BTreeMap<String, String>,
    options: &BTreeMap<String, BTreeSet<String>>,
) -> Result<()> {
    let available = available
        .iter()
        .filter_map(|path| super::super::removal::dlc(path))
        .map(str::to_lowercase)
        .collect();
    for dlc in required {
        ensure!(
            super::super::dependency::Dependency::parse(dlc, format)?
                .matches_options(&available, versions, options)?,
            "Install required DLC '{dlc}' before this package's M3M or TLK steps"
        );
    }
    Ok(())
}

pub(super) fn tlk_jobs(
    changes: &[TlkChange],
    current: &BTreeMap<String, FileIdentity>,
) -> Result<Vec<super::super::helper::Job>> {
    let mut groups: BTreeMap<String, Vec<TlkChange>> = BTreeMap::new();
    for change in changes {
        groups
            .entry(change.target.clone())
            .or_default()
            .push(change.clone());
    }
    let mut jobs = Vec::new();
    for (path, changes) in groups {
        let target = current
            .get(&path)
            .context("Missing staged TLK target")?
            .clone();
        let job = super::super::helper::Job::Tlk {
            targets: vec![target],
            changes,
        };
        job.validate()?;
        jobs.push(job);
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn installation_dependencies_must_exist_at_the_current_step() -> Result<()> {
        let required = vec!["DLC_MOD_A".into()];
        for path in [
            "BioGame/DLC/DLC_MOD_AB/AutoLoad.ini",
            "../system/DLC_MOD_A/a",
            "~docs~/DLC_MOD_A/a",
        ] {
            assert!(
                dependencies(
                    &required,
                    "9.1",
                    &[path.into()].into(),
                    &BTreeMap::new(),
                    &BTreeMap::new()
                )
                .is_err()
            );
        }
        dependencies(
            &required,
            "9.1",
            &["BioGame/DLC/dlc_mod_a/AutoLoad.ini".into()].into(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        )?;
        Ok(())
    }

    // @variants: both
    #[test]
    fn tlk_resolution_ignores_foreign_roots_and_rejects_unmanaged_dlc() -> Result<()> {
        let mut paths: BTreeSet<String> = [
            "../system/Engine.pcc",
            "~docs~/Engine.pcc",
            "Binaries/Engine.pcc",
        ]
        .map(String::from)
        .into();
        assert!(resolve_tlk("Engine.pcc", &paths, &BTreeMap::new(), &BTreeMap::new())?.is_none());
        paths.insert("BioGame/CookedPCConsole/Engine.pcc".into());
        assert_eq!(
            resolve_tlk("Engine.pcc", &paths, &BTreeMap::new(), &BTreeMap::new())?,
            Some("CookedPCConsole/Engine.pcc".into())
        );
        paths.insert("BioGame/DLC/DLC_MOD_Unknown/CookedPCConsole/Engine.pcc".into());
        assert!(resolve_tlk("Engine.pcc", &paths, &BTreeMap::new(), &BTreeMap::new()).is_err());
        Ok(())
    }
}
