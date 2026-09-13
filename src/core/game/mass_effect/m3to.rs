use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod btp;

use super::Target;
use super::manifest::relative_path;
use super::package::{FileMapping, SourceFile};

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
            name.starts_with("TO_")
                && name.len() > 7
                && name.to_ascii_lowercase().ends_with(".pcc"),
            "M3TO source packages must be named TO_<name>.pcc"
        );
        let texture = &entry.textureifp;
        ensure!(
            texture.contains('.')
                && texture.encode_utf16().count() <= 255
                && texture.split('.').all(|part| !part.is_empty()
                    && part.chars().all(|ch| ch.is_alphanumeric() || ch == '_')),
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
            for file in manifests {
                let source = index
                    .get(file.source.as_str())
                    .context("M3TO source is absent from the inspected package")?;
                let manifest = read_manifest(root, source, target)?;
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
        "M3TO source compilation is not supported yet"
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
            let valid = json!({"game":name,"textures":[{"sourcepackage":"Chunk/TO_Test.pcc","textureifp":"Example.Texture"}]});
            parse(&valid.to_string(), game)?;
            for source in [
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
}
