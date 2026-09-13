use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use super::super::Target;
use super::super::{m3da, m3m};
use super::protocol::{Contribution, FileIdentity, Job, M3mKind, M3mOperation, TargetPackage};
use super::{files, tlk_target};

pub(in crate::core::game::mass_effect) const BASES: &[&str] = &[
    "Core.pcc",
    "Engine.pcc",
    "GFxUI.pcc",
    "PlotManagerMap.pcc",
    "SFXOnlineFoundation.pcc",
    "SFXGame.pcc",
    "SFXStrategicAI.pcc",
    "SFXGameContent_Powers.pcc",
];

pub(in crate::core::game::mass_effect) fn compiler_bases(game: Target) -> &'static [&'static str] {
    match game {
        Target::Le1 => BASES,
        Target::Le2 => &[
            "Core.pcc",
            "Engine.pcc",
            "GFxUI.pcc",
            "WwiseAudio.pcc",
            "SFXOnlineFoundation.pcc",
            "PlotManagerMap.pcc",
            "SFXGame.pcc",
            "Startup_INT.pcc",
        ],
        Target::Le3 => &[
            "Core.pcc",
            "Engine.pcc",
            "GameFramework.pcc",
            "GFxUI.pcc",
            "WwiseAudio.pcc",
            "SFXOnlineFoundation.pcc",
            "SFXGame.pcc",
        ],
    }
}

pub(in crate::core::game::mass_effect) fn compiler_inputs(
    game: Target,
    target: &str,
) -> Result<Vec<String>> {
    let name = target
        .strip_prefix("CookedPCConsole/")
        .context("Invalid compiler target")?;
    let bases = compiler_bases(game);
    let index = bases
        .iter()
        .position(|base| *base == name)
        .context("M3M scripts require a supported base package for the selected game")?;
    Ok(bases[..index]
        .iter()
        .map(|name| format!("CookedPCConsole/{name}"))
        .collect())
}

impl Job {
    pub(in crate::core::game::mass_effect) fn validate(&self) -> Result<()> {
        match self {
            Self::Shaders {
                target,
                contributions,
                ..
            } => {
                pairs(std::slice::from_ref(target), 1)?;
                super::super::m3gs::contributions(contributions)?;
            }
            Self::Dlc {
                game,
                inputs,
                outfits,
                emails,
                outputs,
            } => super::dlc_jobs::dlc(*game, inputs, outfits, emails, outputs)?,
            Self::SquadUi { assets, movies } => super::dlc_jobs::ui(assets, movies)?,
            Self::Tlk { targets, changes } => {
                ensure!(
                    !targets.is_empty()
                        && targets.len() <= 64
                        && !changes.is_empty()
                        && changes.len() <= 4096,
                    "Invalid TLK job inputs"
                );
            }
            Self::Plot {
                game,
                target,
                dependencies,
                contributions,
            } => {
                ensure!(
                    matches!(game, Target::Le1 | Target::Le2)
                        && target.original.path == "CookedPCConsole/PlotManager.pcc"
                        && target.current.path == target.original.path
                        && dependencies.len() <= 8
                        && contributions.len() <= 1024,
                    "Invalid plot job inputs"
                );
                let required: BTreeSet<_> = if contributions.is_empty() {
                    BTreeSet::new()
                } else {
                    compiler_bases(*game)
                        .iter()
                        .map(|name| format!("CookedPCConsole/{name}"))
                        .collect()
                };
                ensure!(
                    unique(dependencies.iter())? == required,
                    "Plot compilation requires exactly the declared game base packages"
                );
                let mut mounts = BTreeMap::new();
                let mut dlcs = BTreeMap::new();
                let mut manifests = BTreeSet::new();
                for item in contributions {
                    ensure!(
                        item.dlc.to_ascii_lowercase().starts_with("dlc_mod_")
                            && item.dlc.len() <= 255
                            && item
                                .dlc
                                .bytes()
                                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
                            && mounts
                                .insert(item.mount, item.dlc.to_ascii_lowercase())
                                .is_none_or(|previous| previous == item.dlc.to_ascii_lowercase())
                            && dlcs
                                .insert(item.dlc.to_ascii_lowercase(), item.mount)
                                .is_none_or(|previous| previous == item.mount)
                            && manifests.insert(item.manifest.path.to_ascii_lowercase())
                            && item
                                .manifest
                                .path
                                .strip_prefix(&format!("DLC/{}/CookedPCConsole/", item.dlc))
                                .is_some_and(|name| !name.contains('/') && name.ends_with(".pmu"))
                            && item.manifest.size > 0
                            && item.manifest.size <= 1024 * 1024,
                        "Invalid plot contribution or ambiguous DLC mount order"
                    );
                }
            }
            Self::Tables {
                targets,
                contributions,
            }
            | Self::Config {
                targets,
                contributions,
            } => {
                pairs(targets, 11)?;
                contributions_valid(contributions, matches!(self, Self::Config { .. }))?;
            }
            Self::M3m {
                game,
                targets,
                dependencies,
                assets,
                scripts,
                jobs,
            } => {
                pairs(targets, m3m::TARGETS.len())?;
                ensure!(
                    dependencies.len() <= 8
                        && assets.len() <= 1024
                        && scripts.len() <= 4096
                        && !jobs.is_empty()
                        && jobs.len() <= 4096,
                    "Invalid M3M input counts"
                );
                let targets = unique(targets.iter().map(|target| &target.current))?;
                let dependencies = unique(dependencies.iter())?;
                let asset_names = unique(assets.iter())?;
                let script_names = unique(scripts.iter())?;
                for (inputs, prefix, extension, limit) in [
                    (assets, "Assets/", ".pcc", 128 * 1024 * 1024),
                    (scripts, "Scripts/", ".uc", 4 * 1024 * 1024),
                ] {
                    for input in inputs {
                        ensure!(
                            input.path.starts_with(prefix)
                                && input.path.to_ascii_lowercase().ends_with(extension)
                                && input.size > 0
                                && input.size <= limit,
                            "Invalid M3M asset or script input"
                        );
                    }
                }
                let mut used_targets = BTreeSet::new();
                let mut used_assets = BTreeSet::new();
                let mut used_scripts = BTreeSet::new();
                for job in jobs {
                    ensure!(targets.contains(&job.target), "Undeclared M3M target");
                    entry(&job.entry)?;
                    used_targets.insert(job.target.clone());
                    if job.kind == M3mKind::Asset {
                        entry(&job.source_entry)?;
                        ensure!(asset_names.contains(&job.input), "Undeclared M3M asset");
                        used_assets.insert(job.input.clone());
                    } else {
                        ensure!(
                            script_names.contains(&job.input)
                                && job.source_entry.is_empty()
                                && !job.allow_new
                                && (job.kind != M3mKind::Class
                                    || job.entry.split('.').count() <= 2),
                            "Invalid M3M script job"
                        );
                        used_scripts.insert(job.input.clone());
                    }
                }
                ensure!(
                    targets == used_targets
                        && asset_names == used_assets
                        && script_names == used_scripts,
                    "Unused M3M inputs"
                );
                ensure!(
                    compiler_dependencies(*game, jobs, &targets)? == dependencies,
                    "M3M compiler dependencies do not match the ordered jobs"
                );
            }
        }
        let mut targets = BTreeSet::new();
        for target in self.targets() {
            ensure!(
                targets.insert(target.path.to_ascii_lowercase())
                    && self.accepts_target(&target.path),
                "Invalid or duplicate helper target"
            );
        }
        Ok(())
    }

    fn accepts_target(&self, path: &str) -> bool {
        if matches!(self, Self::SquadUi { .. }) {
            return path
                .strip_prefix(super::super::merge_dlc::COOKED)
                .is_some_and(|name| super::super::merge_dlc::UI_PACKAGES.contains(&name));
        }
        let Some(name) = path.strip_prefix("CookedPCConsole/") else {
            return matches!(self, Self::Tlk { .. }) && tlk_target(path);
        };
        match self {
            Self::Dlc { .. } | Self::SquadUi { .. } => false,
            Self::Tlk { .. } => tlk_target(path),
            Self::Shaders { .. } => path == super::super::m3gs::TARGET,
            Self::Plot { .. } => name == "PlotManager.pcc",
            Self::Tables { .. } => m3da::TARGETS.contains(&name),
            Self::M3m { .. } => m3m::TARGETS.contains(&name),
            Self::Config { .. } => name
                .strip_prefix("Coalesced_")
                .and_then(|name| name.strip_suffix(".bin"))
                .is_some_and(|language| {
                    language.len() == 3 && language.bytes().all(|ch| ch.is_ascii_alphabetic())
                }),
        }
    }
}

fn pairs(targets: &[TargetPackage], max: usize) -> Result<()> {
    ensure!(
        !targets.is_empty()
            && targets.len() <= max
            && targets
                .iter()
                .all(|target| target.original.path == target.current.path),
        "Missing or mismatched original merge targets"
    );
    Ok(())
}

fn unique<'a>(inputs: impl Iterator<Item = &'a FileIdentity>) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let mut folded = BTreeSet::new();
    for input in inputs {
        files::relative(&input.path)?;
        ensure!(
            folded.insert(input.path.to_ascii_lowercase()),
            "Duplicate or case-colliding merge input"
        );
        names.insert(input.path.clone());
    }
    Ok(names)
}

fn entry(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 1024
            && name.split('.').all(|part| !part.is_empty()
                && part
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')),
        "Invalid M3M export name"
    );
    Ok(())
}

pub(super) fn compiler_dependencies(
    game: Target,
    jobs: &[M3mOperation],
    targets: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let mut required = BTreeSet::new();
    for job in jobs.iter().filter(|job| job.kind != M3mKind::Asset) {
        required.extend(compiler_inputs(game, &job.target)?);
    }
    Ok(required.difference(targets).cloned().collect())
}

fn contributions_valid(contributions: &[Contribution], config: bool) -> Result<()> {
    ensure!(contributions.len() <= 4096, "Too many merge contributions");
    let mut manifests = BTreeSet::new();
    let mut mounts = BTreeMap::new();
    let mut dlcs = BTreeMap::new();
    for contribution in contributions {
        let dlc = &contribution.dlc;
        ensure!(
            dlc.to_ascii_lowercase().starts_with("dlc_mod_")
                && dlc.len() <= 255
                && dlc
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_'),
            "Invalid custom DLC identity"
        );
        let folded = dlc.to_ascii_lowercase();
        ensure!(
            mounts
                .insert(contribution.mount, folded.clone())
                .is_none_or(|previous| previous == folded)
                && dlcs
                    .insert(folded, contribution.mount)
                    .is_none_or(|previous| previous == contribution.mount),
            "Ambiguous DLC mount ordering"
        );
        files::relative(&contribution.manifest.path)?;
        let path = contribution.manifest.path.to_ascii_lowercase();
        let prefix = format!("DLC/{dlc}/CookedPCConsole/").to_ascii_lowercase();
        let name = path
            .strip_prefix(&prefix)
            .context("Merge manifest is outside its DLC")?;
        ensure!(
            !name.contains('/')
                && contribution.manifest.size > 0
                && contribution.manifest.size <= 1024 * 1024
                && manifests.insert(path.clone()),
            "Invalid or duplicate merge manifest"
        );
        if config {
            ensure!(
                name.starts_with("configdelta-")
                    && name.ends_with(".m3cd")
                    && name.len() > 17
                    && contribution.packages.is_empty(),
                "Invalid M3CD contribution"
            );
        } else {
            ensure!(
                name.starts_with(&format!("{}-", dlc.to_ascii_lowercase()))
                    && name.ends_with(".m3da")
                    && name.len() > dlc.len() + 6
                    && !contribution.packages.is_empty()
                    && contribution.packages.len() <= 1024,
                "Invalid M3DA contribution"
            );
            let mut packages = BTreeSet::new();
            for package in &contribution.packages {
                files::relative(&package.path)?;
                let path = package.path.to_ascii_lowercase();
                let name = path
                    .strip_prefix(&prefix)
                    .context("M3DA source package is outside its DLC")?;
                ensure!(
                    !name.contains('/')
                        && name.ends_with(".pcc")
                        && packages.insert(name.to_string()),
                    "Invalid or ambiguous M3DA source package"
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str) -> FileIdentity {
        FileIdentity {
            path: path.into(),
            size: 10,
            sha256: "0".repeat(64),
        }
    }
    fn target(path: &str) -> TargetPackage {
        TargetPackage {
            original: input(path),
            current: input(path),
        }
    }
    fn contribution(dlc: &str, name: &str) -> Contribution {
        Contribution {
            dlc: dlc.into(),
            mount: 5,
            manifest: input(&format!("DLC/{dlc}/CookedPCConsole/{name}")),
            packages: Vec::new(),
        }
    }

    // @variants: both
    #[test]
    fn rejects_plot_dependency_and_contribution_mismatches_before_execution() -> Result<()> {
        let contribution = super::super::protocol::PlotContribution {
            dlc: "DLC_MOD_A".into(),
            mount: 5,
            manifest: input("DLC/DLC_MOD_A/CookedPCConsole/PlotManagerUpdate.pmu"),
        };
        let dependencies: Vec<_> = BASES
            .iter()
            .map(|name| input(&format!("CookedPCConsole/{name}")))
            .collect();
        let job = |dependencies, contributions| Job::Plot {
            game: Target::Le1,
            target: target("CookedPCConsole/PlotManager.pcc"),
            dependencies,
            contributions,
        };
        job(dependencies.clone(), vec![contribution.clone()]).validate()?;
        let mut extra = contribution.clone();
        extra.manifest.path = "DLC/DLC_MOD_A/CookedPCConsole/Extra.pmu".into();
        job(
            dependencies.clone(),
            vec![contribution.clone(), extra.clone()],
        )
        .validate()?;
        extra.mount += 1;
        assert!(
            job(
                dependencies.clone(),
                vec![contribution.clone(), extra.clone()]
            )
            .validate()
            .is_err()
        );
        extra.mount = contribution.mount;
        extra.dlc = "DLC_MOD_B".into();
        extra.manifest.path = "DLC/DLC_MOD_B/CookedPCConsole/Extra.pmu".into();
        assert!(
            job(dependencies.clone(), vec![contribution.clone(), extra])
                .validate()
                .is_err()
        );
        job(Vec::new(), Vec::new()).validate()?;
        assert!(
            job(Vec::new(), vec![contribution.clone()])
                .validate()
                .is_err()
        );
        assert!(job(dependencies.clone(), Vec::new()).validate().is_err());
        assert!(
            job(
                dependencies.clone(),
                vec![contribution.clone(), contribution.clone()]
            )
            .validate()
            .is_err()
        );
        let mut invalid = contribution;
        invalid.manifest.path = "DLC/DLC_MOD_B/CookedPCConsole/PlotManagerUpdate.pmu".into();
        assert!(job(dependencies, vec![invalid]).validate().is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn restricts_each_merge_type_to_its_own_targets_and_originals() -> Result<()> {
        for path in [
            "CookedPCConsole/Engine.pcc",
            "CookedPCConsole/Coalesced_INT.bin",
            "CookedPCConsole/Startup_INT.pcc",
            "../Engine.pcc",
            "../system/a",
            "~docs~/a",
            "Mods/a",
        ] {
            let tables = Job::Tables {
                targets: vec![target(path)],
                contributions: Vec::new(),
            };
            let config = Job::Config {
                targets: vec![target(path)],
                contributions: Vec::new(),
            };
            assert_eq!(
                tables.validate().is_ok(),
                path == "CookedPCConsole/Engine.pcc",
                "{path}"
            );
            assert_eq!(
                config.validate().is_ok(),
                path == "CookedPCConsole/Coalesced_INT.bin",
                "{path}"
            );
        }
        let mut mismatch = target("CookedPCConsole/Engine.pcc");
        mismatch.original.path = "CookedPCConsole/SFXGame.pcc".into();
        assert!(
            Job::Tables {
                targets: vec![mismatch],
                contributions: Vec::new()
            }
            .validate()
            .is_err()
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn accepts_multiple_manifests_per_dlc_but_rejects_ambiguous_mounts() -> Result<()> {
        let first = contribution("DLC_MOD_A", "ConfigDelta-first.m3cd");
        let second = contribution("DLC_MOD_A", "ConfigDelta-second.m3cd");
        let job = |items| Job::Config {
            targets: vec![target("CookedPCConsole/Coalesced_INT.bin")],
            contributions: items,
        };
        job(vec![first.clone(), second.clone()]).validate()?;
        assert!(job(vec![first.clone(), first.clone()]).validate().is_err());
        let mut other = second.clone();
        other.mount = 6;
        assert!(job(vec![first.clone(), other]).validate().is_err());
        assert!(
            job(vec![
                first.clone(),
                contribution("DLC_MOD_B", "ConfigDelta-other.m3cd")
            ])
            .validate()
            .is_err()
        );
        let mut bad = second;
        bad.packages.push(input("CookedPCConsole/Engine.pcc"));
        assert!(job(vec![bad]).validate().is_err());
        let mut table = contribution("DLC_MOD_A", "DLC_MOD_A-table.m3da");
        table
            .packages
            .push(input("DLC/DLC_MOD_A/CookedPCConsole/Table.pcc"));
        Job::Tables {
            targets: vec![target("CookedPCConsole/Engine.pcc")],
            contributions: vec![table.clone()],
        }
        .validate()?;
        table.packages[0].path = "DLC/DLC_MOD_B/CookedPCConsole/Table.pcc".into();
        assert!(
            Job::Tables {
                targets: vec![target("CookedPCConsole/Engine.pcc")],
                contributions: vec![table]
            }
            .validate()
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn requires_exact_compiler_dependencies_and_used_assets() -> Result<()> {
        let operation = M3mOperation {
            target: "CookedPCConsole/Engine.pcc".into(),
            entry: "Class.Fn".into(),
            kind: M3mKind::Function,
            input: "Scripts/example.uc".into(),
            source_entry: String::new(),
            allow_new: false,
        };
        let build = |dependencies, assets, jobs| Job::M3m {
            game: crate::core::game::mass_effect::Target::Le1,
            targets: vec![target("CookedPCConsole/Engine.pcc")],
            dependencies,
            assets,
            scripts: vec![input("Scripts/example.uc")],
            jobs,
        };
        build(
            vec![input("CookedPCConsole/Core.pcc")],
            Vec::new(),
            vec![operation.clone()],
        )
        .validate()?;
        assert!(
            build(Vec::new(), Vec::new(), vec![operation.clone()])
                .validate()
                .is_err()
        );
        assert!(
            build(
                vec![input("CookedPCConsole/Core.pcc")],
                vec![input("Assets/unused.pcc")],
                vec![operation.clone()]
            )
            .validate()
            .is_err()
        );
        let mut bad = operation;
        bad.allow_new = true;
        assert!(
            build(
                vec![input("CookedPCConsole/Core.pcc")],
                Vec::new(),
                vec![bad]
            )
            .validate()
            .is_err()
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn compiler_inputs_follow_each_games_package_order() -> Result<()> {
        for (game, expected) in [
            (
                Target::Le1,
                vec![
                    "Core",
                    "Engine",
                    "GFxUI",
                    "PlotManagerMap",
                    "SFXOnlineFoundation",
                ],
            ),
            (
                Target::Le2,
                vec![
                    "Core",
                    "Engine",
                    "GFxUI",
                    "WwiseAudio",
                    "SFXOnlineFoundation",
                    "PlotManagerMap",
                ],
            ),
            (
                Target::Le3,
                vec![
                    "Core",
                    "Engine",
                    "GameFramework",
                    "GFxUI",
                    "WwiseAudio",
                    "SFXOnlineFoundation",
                ],
            ),
        ] {
            let expected: Vec<_> = expected
                .into_iter()
                .map(|name| format!("CookedPCConsole/{name}.pcc"))
                .collect();
            assert_eq!(
                compiler_inputs(game, "CookedPCConsole/SFXGame.pcc")?,
                expected
            );
            assert!(compiler_inputs(game, "CookedPCConsole/Core.pcc")?.is_empty());
            for invalid in [
                "../SFXGame.pcc",
                "../system/SFXGame.pcc",
                "~docs~/SFXGame.pcc",
                "Mods/SFXGame.pcc",
                "CookedPCConsole/../SFXGame.pcc",
            ] {
                assert!(compiler_inputs(game, invalid).is_err());
            }
        }
        assert!(compiler_inputs(Target::Le1, "CookedPCConsole/Startup_INT.pcc").is_err());
        assert!(compiler_inputs(Target::Le3, "CookedPCConsole/Startup_INT.pcc").is_err());
        assert_eq!(
            compiler_inputs(Target::Le2, "CookedPCConsole/Startup_INT.pcc")?
                .last()
                .map(String::as_str),
            Some("CookedPCConsole/SFXGame.pcc")
        );
        assert!(compiler_inputs(Target::Le2, "CookedPCConsole/GameFramework.pcc").is_err());
        assert!(compiler_inputs(Target::Le3, "CookedPCConsole/PlotManagerMap.pcc").is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn compiler_dependencies_reuse_targets_and_ignore_asset_only_jobs() -> Result<()> {
        let script = M3mOperation {
            target: "CookedPCConsole/SFXGame.pcc".into(),
            entry: "Class.Fn".into(),
            kind: M3mKind::Function,
            input: "Scripts/example.uc".into(),
            source_entry: String::new(),
            allow_new: false,
        };
        let asset = M3mOperation {
            target: "CookedPCConsole/EntryMenu.pcc".into(),
            kind: M3mKind::Asset,
            ..script.clone()
        };
        for game in [Target::Le1, Target::Le2, Target::Le3] {
            let targets: BTreeSet<String> = ["CookedPCConsole/Engine.pcc".into()].into();
            let expected: BTreeSet<_> = compiler_inputs(game, &script.target)?
                .into_iter()
                .filter(|path| !targets.contains(path))
                .collect();
            assert_eq!(
                compiler_dependencies(
                    game,
                    &[script.clone(), script.clone(), asset.clone()],
                    &targets
                )?,
                expected
            );
            assert!(
                compiler_dependencies(game, std::slice::from_ref(&asset), &BTreeSet::new())?
                    .is_empty()
            );
        }
        Ok(())
    }
}
