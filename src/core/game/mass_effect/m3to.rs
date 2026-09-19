use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod btp;

use super::Target;
use super::helper::{FileIdentity, Job};
use super::manifest::relative_path;
use super::package::{FileMapping, SourceFile};
use super::recipe::PlannedFile;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct TextureOverride {
    pub(super) source: String,
    pub(super) texture: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SourceManifest {
    pub(super) source: String,
    pub(super) textures: Vec<TextureOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Layout {
    Compile { manifests: Vec<SourceManifest> },
    Precompiled { package: String, metadata: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct M3toPlan {
    pub(super) dlc: String,
    pub(super) layout: Layout,
    mount: i32,
    textures: BTreeSet<String>,
}

impl M3toPlan {
    pub(super) fn dlc(&self) -> &str {
        &self.dlc
    }

    pub(super) fn is_compiled(&self) -> bool {
        matches!(self.layout, Layout::Precompiled { .. })
    }
}

pub(super) struct Compilation {
    pub(super) job: Job,
    pub(super) removals: Vec<String>,
    pub(super) outputs: BTreeSet<String>,
    textures: Vec<String>,
}

impl Compilation {
    pub(super) fn bind(&mut self, files: &BTreeMap<String, FileIdentity>) -> Result<()> {
        let Job::Texture {
            manifests,
            packages,
            ..
        } = &mut self.job
        else {
            bail!("Invalid M3TO compilation job")
        };
        for input in manifests.iter_mut().chain(packages) {
            *input = files
                .get(&input.path)
                .with_context(|| format!("Missing final M3TO input '{}'", input.path))?
                .clone();
        }
        self.job.validate()
    }
}

fn planned_input<'a>(source: &str, files: &'a [PlannedFile]) -> Result<&'a PlannedFile> {
    let mut matches = files.iter().filter(|file| file.source == source);
    let file = matches
        .next()
        .with_context(|| format!("Missing staged M3TO input '{source}'"))?;
    ensure!(
        matches.next().is_none(),
        "M3TO input '{source}' has ambiguous destinations"
    );
    Ok(file)
}

fn identity(file: &PlannedFile) -> Result<FileIdentity> {
    Ok(FileIdentity {
        path: file
            .destination
            .relative
            .strip_prefix("BioGame/")
            .context("M3TO input is outside BioGame")?
            .to_owned(),
        size: file.destination.size,
        sha256: file.destination.sha256.clone(),
    })
}

pub(super) fn compilations(
    plans: &[M3toPlan],
    files: &[PlannedFile],
    game: Target,
) -> Result<Vec<Compilation>> {
    let mut result = Vec::new();
    for plan in plans {
        let Layout::Compile { manifests } = &plan.layout else {
            continue;
        };
        let mut manifest_inputs = Vec::new();
        let mut package_inputs = BTreeMap::new();
        let mut removals = BTreeSet::new();
        let mut texture_paths = Vec::new();
        for manifest in manifests {
            let file = planned_input(&manifest.source, files)?;
            removals.insert(file.destination.relative.clone());
            manifest_inputs.push(identity(file)?);
            for texture in &manifest.textures {
                if let Some(index) = texture_paths
                    .iter()
                    .position(|name: &String| name.eq_ignore_ascii_case(&texture.texture))
                {
                    texture_paths.remove(index);
                }
                texture_paths.push(texture.texture.clone());
                let file = planned_input(&texture.source, files)?;
                removals.insert(file.destination.relative.clone());
                package_inputs
                    .entry(file.destination.relative.to_lowercase())
                    .or_insert(identity(file)?);
            }
        }
        let outputs = vec![
            format!("DLC/{}/CombinedTextureOverrides.btp", plan.dlc),
            format!("DLC/{}/BTPMetadata.btm", plan.dlc),
        ];
        let output_set = outputs.iter().cloned().collect();
        let job = Job::Texture {
            game,
            dlc: plan.dlc.clone(),
            manifests: manifest_inputs,
            packages: package_inputs.into_values().collect(),
            outputs,
            textures: u32::try_from(texture_paths.len()).context("Too many M3TO textures")?,
        };
        job.validate()?;
        result.push(Compilation {
            job,
            removals: removals.into_iter().collect(),
            outputs: output_set,
            textures: texture_paths,
        });
    }
    Ok(result)
}

pub(super) fn combine(compilations: Vec<Compilation>) -> Result<Vec<Compilation>> {
    let mut groups: BTreeMap<String, Compilation> = BTreeMap::new();
    for mut compilation in compilations {
        let Job::Texture {
            game,
            dlc,
            manifests,
            packages,
            outputs,
            ..
        } = &mut compilation.job
        else {
            bail!("Invalid M3TO compilation job")
        };
        let key = dlc.to_lowercase();
        let Some(existing) = groups.get_mut(&key) else {
            groups.insert(key, compilation);
            continue;
        };
        let Job::Texture {
            game: existing_game,
            manifests: existing_manifests,
            packages: existing_packages,
            textures,
            ..
        } = &mut existing.job
        else {
            bail!("Invalid M3TO compilation group")
        };
        ensure!(
            existing_game == game && existing.outputs.iter().eq(outputs.iter()),
            "Conflicting M3TO compilation targets"
        );
        for manifest in manifests.drain(..) {
            existing_manifests.retain(|item| !item.path.eq_ignore_ascii_case(&manifest.path));
            existing_manifests.push(manifest);
        }
        for package in packages.drain(..) {
            existing_packages.retain(|item| !item.path.eq_ignore_ascii_case(&package.path));
            existing_packages.push(package);
        }
        for texture in compilation.textures {
            existing
                .textures
                .retain(|item| !item.eq_ignore_ascii_case(&texture));
            existing.textures.push(texture);
        }
        *textures = u32::try_from(existing.textures.len()).context("Too many M3TO textures")?;
        existing.removals.extend(compilation.removals);
        existing.removals.sort();
        existing.removals.dedup();
        existing.job.validate()?;
    }
    Ok(groups.into_values().collect())
}

pub(super) fn validate_job(
    game: Target,
    dlc: &str,
    manifests: &[FileIdentity],
    packages: &[FileIdentity],
    outputs: &[String],
    textures: u32,
) -> Result<()> {
    super::manifest::validate_dlc(dlc)?;
    ensure!(
        dlc.starts_with("DLC_MOD_")
            && !game.is_official_dlc(dlc)
            && !manifests.is_empty()
            && manifests.len() <= 64
            && !packages.is_empty()
            && packages.len() <= 4096
            && (1..=100_000).contains(&textures),
        "Invalid M3TO compilation request"
    );
    let cooked = format!("DLC/{dlc}/CookedPCConsole/");
    let mut paths = BTreeSet::new();
    for manifest in manifests {
        let name = manifest.path.strip_prefix(&cooked).unwrap_or_default();
        ensure!(
            !name.contains('/')
                && name.starts_with("TextureOverride-")
                && name.len() > 21
                && name.to_ascii_lowercase().ends_with(".m3to")
                && manifest.size > 0
                && manifest.size <= 16 * 1024 * 1024
                && paths.insert(manifest.path.to_lowercase()),
            "Invalid or duplicate M3TO manifest input"
        );
    }
    for package in packages {
        let name = package.path.strip_prefix(&cooked).unwrap_or_default();
        ensure!(
            !name.contains('/')
                && name.starts_with("TO_")
                && name.len() > 7
                && name.to_ascii_lowercase().ends_with(".pcc")
                && package.size > 0
                && package.size <= 512 * 1024 * 1024
                && paths.insert(package.path.to_lowercase()),
            "Invalid or duplicate M3TO package input"
        );
    }
    let total = manifests
        .iter()
        .chain(packages)
        .try_fold(0_u64, |total, input| total.checked_add(input.size))
        .context("M3TO input size overflow")?;
    ensure!(total <= 4 * 1024 * 1024 * 1024, "M3TO inputs exceed 4 GiB");
    ensure!(
        outputs
            == [
                format!("DLC/{dlc}/CombinedTextureOverrides.btp"),
                format!("DLC/{dlc}/BTPMetadata.btm"),
            ],
        "M3TO output inventory differs from its plan"
    );
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    game: Target,
    textures: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    sourcepackage: String,
    textureifp: String,
}

fn parse(text: &str, target: Target) -> Result<Manifest> {
    ensure!(
        text.len() <= 16 * 1024 * 1024,
        "M3TO manifest exceeds 16 MiB"
    );
    let manifest: Manifest =
        serde_json::from_str(text).context("Invalid or unsupported M3TO manifest")?;
    ensure!(
        manifest.game == target,
        "M3TO targets a different game than moddesc.ini"
    );
    ensure!(
        !manifest.textures.is_empty() && manifest.textures.len() <= 100_000,
        "Invalid M3TO texture count"
    );
    let mut names = BTreeSet::new();
    for entry in &manifest.textures {
        let package = relative_path(&entry.sourcepackage)?;
        let name = package
            .rsplit('/')
            .next()
            .context("Missing M3TO source package name")?;
        ensure!(
            package == name
                && name.starts_with("TO_")
                && name.len() > 7
                && name.to_ascii_lowercase().ends_with(".pcc"),
            "M3TO source packages must be named TO_<name>.pcc"
        );
        let texture = &entry.textureifp;
        ensure!(
            texture.contains('.')
                && texture.encode_utf16().count() <= 255
                && texture.split('.').all(|part| !part.is_empty()
                    && part
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')),
            "M3TO texture paths must identify a nested export and fit the runtime's 255-character limit"
        );
        ensure!(
            names.insert(texture.to_lowercase()),
            "Duplicate texture override '{texture}' in one M3TO manifest"
        );
    }
    Ok(manifest)
}

fn read_manifest(root: &Path, source: &SourceFile, target: Target) -> Result<Manifest> {
    ensure!(
        source.size <= 16 * 1024 * 1024,
        "M3TO manifest exceeds 16 MiB"
    );
    let mut bytes = Vec::new();
    File::open(root.join(&source.relative))?
        .take(source.size + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 == source.size
            && format!("{:x}", Sha256::digest(&bytes)) == source.sha256,
        "M3TO manifest changed during inspection"
    );
    parse(
        std::str::from_utf8(&bytes).context("M3TO manifest must be UTF-8")?,
        target,
    )
}

pub(super) fn inspect(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
    target: Target,
    format: &str,
) -> Result<Vec<M3toPlan>> {
    inspect_controlled(
        root,
        sources,
        files,
        target,
        format,
        &super::operation::Control::recovery(),
    )
}

fn inspect_controlled(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
    target: Target,
    format: &str,
    control: &super::operation::Control,
) -> Result<Vec<M3toPlan>> {
    let mut groups: BTreeMap<String, Vec<&FileMapping>> = BTreeMap::new();
    for file in files.iter().filter(|file| {
        matches!(
            super::package::transformation(&file.destination),
            Some(
                super::package::Transformation::TextureOverride
                    | super::package::Transformation::PrecompiledTextureOverride
            )
        )
    }) {
        ensure!(format == "9.2", "M3TO requires moddesc 9.2");
        let parts: Vec<_> = file.destination.split('/').collect();
        ensure!(
            parts.len() >= 3
                && parts[0].eq_ignore_ascii_case("DLC")
                && parts[1].starts_with("DLC_MOD_"),
            "M3TO inputs must belong to a custom DLC"
        );
        let valid = if file.destination.to_ascii_lowercase().ends_with(".m3to") {
            parts.len() == 4
                && parts[2].eq_ignore_ascii_case("CookedPCConsole")
                && parts[3].starts_with("TextureOverride-")
                && parts[3].len() > 21
        } else {
            parts.len() == 3
                && ["CombinedTextureOverrides.btp", "BTPMetadata.btm"]
                    .iter()
                    .any(|name| parts[2].eq_ignore_ascii_case(name))
        };
        ensure!(
            valid,
            "M3TO requires TextureOverride-<name>.m3to in CookedPCConsole, or CombinedTextureOverrides.btp and BTPMetadata.btm at the DLC root"
        );
        groups.entry(parts[1].into()).or_default().push(file);
    }
    let index: BTreeMap<_, _> = sources
        .iter()
        .map(|file| (file.relative.as_str(), file))
        .collect();
    let destinations: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.destination.to_lowercase(), file))
        .collect();
    let mut plans = Vec::new();
    for (dlc, group) in groups {
        control.check()?;
        let manifests: Vec<_> = group
            .iter()
            .filter(|file| file.destination.to_ascii_lowercase().ends_with(".m3to"))
            .collect();
        let mut textures = BTreeSet::new();
        let layout = if manifests.is_empty() {
            let find = |name: &str| -> Result<String> {
                let file = destinations
                    .get(&format!("DLC/{dlc}/{name}").to_lowercase())
                    .with_context(|| format!("Precompiled M3TO is missing {name}"))?;
                Ok(file.source.clone())
            };
            ensure!(
                !files.iter().any(|file| file
                    .destination
                    .to_lowercase()
                    .starts_with(&format!("DLC/{dlc}/CookedPCConsole/").to_lowercase())
                    && file
                        .destination
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| name.starts_with("TO_")
                            && name.to_ascii_lowercase().ends_with(".pcc"))),
                "Precompiled M3TO cannot include source TO_ packages in the same DLC"
            );
            let package = find("CombinedTextureOverrides.btp")?;
            let metadata = find("BTPMetadata.btm")?;
            textures = btp::inspect(
                root,
                index.get(package.as_str()).context("Missing BTP source")?,
                index.get(metadata.as_str()).context("Missing BTM source")?,
                &dlc,
                target,
                |name| {
                    let destination =
                        format!("DLC/{dlc}/CookedPCConsole/{name}.tfc").to_lowercase();
                    let mapping = destinations
                        .get(&destination)
                        .with_context(|| format!("Missing M3TO TFC '{name}'"))?;
                    Ok((*index
                        .get(mapping.source.as_str())
                        .context("Missing TFC source")?)
                    .clone())
                },
                control,
            )?;
            Layout::Precompiled { package, metadata }
        } else {
            ensure!(
                manifests.len() == group.len(),
                "M3TO source manifests and precompiled BTP/BTM files cannot be combined in the same DLC"
            );
            let mut result = Vec::new();
            let mut total_textures = 0_usize;
            for file in manifests {
                let source = index
                    .get(file.source.as_str())
                    .context("M3TO source is absent from the inspected package")?;
                let manifest = read_manifest(root, source, target)?;
                total_textures = total_textures
                    .checked_add(manifest.textures.len())
                    .context("M3TO texture count overflow")?;
                ensure!(
                    total_textures <= 100_000,
                    "M3TO texture count exceeds 100000"
                );
                let mut textures = Vec::new();
                for entry in manifest.textures {
                    let package = relative_path(&entry.sourcepackage)?;
                    let destination = format!("DLC/{dlc}/CookedPCConsole/{package}");
                    let source = destinations.get(&destination.to_lowercase()).with_context(
                        || {
                            format!(
                                "M3TO source package '{package}' is not installed in DLC '{dlc}'"
                            )
                        },
                    )?;
                    textures.push(TextureOverride {
                        source: source.source.clone(),
                        texture: entry.textureifp,
                    });
                }
                result.push(SourceManifest {
                    source: file.source.clone(),
                    textures,
                });
            }
            Layout::Compile { manifests: result }
        };
        if let Layout::Compile { manifests } = &layout {
            textures.extend(manifests.iter().flat_map(|manifest| {
                manifest
                    .textures
                    .iter()
                    .map(|texture| texture.texture.to_lowercase())
            }));
        }
        let path = format!(
            "DLC/{dlc}/{}",
            if target == Target::Le1 {
                "AutoLoad.ini"
            } else {
                "CookedPCConsole/Mount.dlc"
            }
        );
        let mapping = destinations
            .get(&path.to_lowercase())
            .context("Texture overrides require the DLC mount file")?;
        let source = index
            .get(mapping.source.as_str())
            .context("Missing DLC mount source")?;
        let mount = mount(root, source, target)?;
        plans.push(M3toPlan {
            dlc,
            layout,
            mount,
            textures,
        });
    }
    let mut precedence = BTreeMap::new();
    for plan in &plans {
        for texture in &plan.textures {
            if let Some(previous) = precedence.insert((plan.mount, texture), &plan.dlc) {
                anyhow::bail!(
                    "Texture override '{texture}' has ambiguous DLC mount priority in '{previous}' and '{}'; use compatible mod versions",
                    plan.dlc
                );
            }
        }
    }
    Ok(plans)
}

pub(super) fn mount(root: &Path, source: &SourceFile, target: Target) -> Result<i32> {
    ensure!(source.size <= 1024 * 1024, "DLC mount file exceeds 1 MiB");
    let mut data = Vec::new();
    File::open(root.join(&source.relative))?
        .take(source.size + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 == source.size && format!("{:x}", Sha256::digest(&data)) == source.sha256,
        "DLC mount file changed during validation"
    );
    let value = if target == Target::Le1 {
        let text = std::str::from_utf8(&data).context("Invalid LE1 DLC mount encoding")?;
        let value = super::m3da::mount(text)?;
        let runtime_value = text.lines().find_map(|line| {
            let (key, raw) = line.split_once('=')?;
            if key.trim() != "ModMount" {
                return None;
            }
            let raw = raw.trim_start();
            let end = raw
                .char_indices()
                .find(|(index, ch)| {
                    !(ch.is_ascii_digit() || *index == 0 && matches!(ch, '+' | '-'))
                })
                .map_or(raw.len(), |(index, _)| index);
            raw[..end].parse::<i32>().ok().filter(|value| *value >= 0)
        });
        ensure!(
            runtime_value == Some(value),
            "LE1 texture runtime requires an unambiguous ModMount line with this exact key casing"
        );
        value
    } else {
        let (length, offset, versions) = if target == Target::Le2 {
            (44, 12, [684, 168, 65643])
        } else {
            (36, 16, [685, 205, 196715])
        };
        ensure!(data.len() >= length, "Truncated DLC mount file");
        let version_offset = if target == Target::Le2 { 0 } else { 4 };
        for (index, version) in versions.into_iter().enumerate() {
            ensure!(
                data[version_offset + index * 4..version_offset + index * 4 + 4]
                    == u32::to_le_bytes(version),
                "DLC mount file targets a different game or unsupported format"
            );
        }
        i32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    };
    ensure!(value >= 0, "DLC mount priority must be nonnegative");
    Ok(value)
}

pub(super) fn validate_staged(
    root: &Path,
    sources: &[SourceFile],
    target: Target,
    control: &super::operation::Control,
) -> Result<()> {
    let files: Vec<_> = sources
        .iter()
        .filter_map(|file| {
            file.relative
                .strip_prefix("BioGame/")
                .map(|destination| FileMapping {
                    source: file.relative.clone(),
                    destination: destination.into(),
                })
        })
        .collect();
    let plans = inspect_controlled(root, sources, &files, target, "9.2", control)?;
    ensure!(
        plans
            .iter()
            .all(|plan| matches!(plan.layout, Layout::Precompiled { .. })),
        "Raw M3TO inputs remained in the final deployment candidate"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // @variants: both
    #[test]
    fn validates_game_paths_versions_and_unknown_texture_semantics() -> Result<()> {
        for (game, name) in [
            (Target::Le1, "LE1"),
            (Target::Le2, "LE2"),
            (Target::Le3, "LE3"),
        ] {
            let valid = json!({"game":name,"textures":[{"sourcepackage":"TO_Test.pcc","textureifp":"Example.Texture"}]});
            parse(&valid.to_string(), game)?;
            for source in [
                "Chunk/TO_Test.pcc",
                "../TO_Test.pcc",
                "Engine.pcc",
                "/TO_Test.pcc",
                "TO_Test.dll",
            ] {
                let mut invalid = valid.clone();
                invalid["textures"][0]["sourcepackage"] = source.into();
                assert!(parse(&invalid.to_string(), game).is_err(), "{source}");
            }
            for texture in ["RootTexture", "A..B", "A/B.C", "A.B\\C"] {
                let mut invalid = valid.clone();
                invalid["textures"][0]["textureifp"] = texture.into();
                assert!(parse(&invalid.to_string(), game).is_err());
            }
            let mut invalid = valid.clone();
            invalid["textures"][0]["future"] = true.into();
            assert!(parse(&invalid.to_string(), game).is_err());
            invalid = valid.clone();
            invalid["game"] = "ME1".into();
            assert!(parse(&invalid.to_string(), game).is_err());
            invalid = valid.clone();
            invalid["textures"] = json!([valid["textures"][0], valid["textures"][0]]);
            assert!(parse(&invalid.to_string(), game).is_err());
        }
        Ok(())
    }

    #[test]
    fn helper_job_requires_bounded_custom_dlc_inputs_and_exact_outputs() {
        let manifest = FileIdentity {
            path: "DLC/DLC_MOD_Test/CookedPCConsole/TextureOverride-Test.m3to".into(),
            size: 10,
            sha256: "a".repeat(64),
        };
        let package = FileIdentity {
            path: "DLC/DLC_MOD_Test/CookedPCConsole/TO_Test.pcc".into(),
            size: 20,
            sha256: "b".repeat(64),
        };
        let outputs = vec![
            "DLC/DLC_MOD_Test/CombinedTextureOverrides.btp".into(),
            "DLC/DLC_MOD_Test/BTPMetadata.btm".into(),
        ];
        assert!(
            validate_job(
                Target::Le2,
                "DLC_MOD_Test",
                &[manifest.clone()],
                &[package.clone()],
                &outputs,
                1
            )
            .is_ok()
        );
        assert!(
            validate_job(
                Target::Le2,
                "DLC_EXP_Test",
                &[manifest.clone()],
                &[package.clone()],
                &outputs,
                1
            )
            .is_err()
        );
        let oversized = FileIdentity {
            size: 512 * 1024 * 1024 + 1,
            ..package
        };
        assert!(
            validate_job(
                Target::Le2,
                "DLC_MOD_Test",
                &[manifest],
                &[oversized],
                &outputs,
                1
            )
            .is_err()
        );
    }
}
