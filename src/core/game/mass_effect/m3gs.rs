use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::Target;
use super::helper::protocol::ShaderContribution;
use super::package::{FileMapping, SourceFile};

pub(super) const TARGET: &str = "CookedPCConsole/GlobalShaderCache-PC-D3D-SM5.bin";
pub(super) const LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Plan {
    pub(super) source: SourceFile,
    pub(super) destination: String,
    pub(super) dlc: String,
    pub(super) index: u32,
}

pub(super) fn location(path: &str) -> Result<(&str, u32)> {
    let parts: Vec<_> = path.split('/').collect();
    ensure!(
        parts.len() == 4
            && parts[0].eq_ignore_ascii_case("DLC")
            && parts[1].to_ascii_lowercase().starts_with("dlc_mod_")
            && parts[1].len() <= 255
            && parts[1]
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
            && parts[2].eq_ignore_ascii_case("CookedPCConsole"),
        "M3GS requires a custom DLC CookedPCConsole directory"
    );
    let name = parts[3].to_ascii_lowercase();
    let number = name
        .strip_prefix("globalshader-")
        .and_then(|name| name.strip_suffix(".m3gs"))
        .and_then(|name| name.split_once('-'))
        .filter(|(_, label)| !label.is_empty())
        .map(|(number, _)| number)
        .context("M3GS requires a GlobalShader-<index>-<name>.m3gs filename")?;
    ensure!(
        !number.is_empty() && number.bytes().all(|ch| ch.is_ascii_digit()),
        "Invalid M3GS shader index"
    );
    let index: u32 = number.parse().context("Invalid M3GS shader index")?;
    ensure!(
        index <= i32::MAX as u32,
        "M3GS shader index exceeds its limit"
    );
    Ok((parts[1], index))
}

pub(super) fn bytecode(data: &[u8]) -> Result<()> {
    ensure!(
        data.len() >= 32 && data.len() as u64 <= LIMIT && &data[..4] == b"DXBC",
        "M3GS requires compiled DXBC shader bytecode"
    );
    let word = |offset: usize| -> Result<usize> {
        Ok(u32::from_le_bytes(
            data.get(offset..offset + 4)
                .context("Truncated M3GS chunk")?
                .try_into()?,
        ) as usize)
    };
    let count = word(28)?;
    ensure!(
        word(20)? == 1 && word(24)? == data.len() && count > 0 && count <= (data.len() - 32) / 4,
        "Invalid M3GS DXBC header"
    );
    let end = 32 + count * 4;
    let mut chunks = Vec::new();
    for index in 0..count {
        let offset = word(32 + index * 4)?;
        ensure!(
            offset >= end && offset <= data.len() - 8,
            "M3GS chunk is outside its payload"
        );
        let size = word(offset + 4)?;
        ensure!(
            size <= data.len() - offset - 8,
            "Truncated M3GS chunk payload"
        );
        chunks.push((offset, offset + 8 + size));
    }
    chunks.sort_unstable();
    ensure!(
        chunks.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "Overlapping M3GS chunks"
    );
    Ok(())
}

pub(super) fn inspect(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
    game: Target,
    format: &str,
) -> Result<Vec<Plan>> {
    let mut plans = Vec::new();
    let mut indices = BTreeSet::new();
    for file in files
        .iter()
        .filter(|file| file.destination.to_ascii_lowercase().ends_with(".m3gs"))
    {
        ensure!(format == "9.2", "M3GS requires moddesc 9.2");
        let (dlc, index) = location(&file.destination)?;
        ensure!(
            indices.insert((dlc.to_ascii_lowercase(), index)),
            "Multiple M3GS files replace shader {index} within DLC '{dlc}'"
        );
        let mount = if game == Target::Le1 {
            format!("DLC/{dlc}/AutoLoad.ini")
        } else {
            format!("DLC/{dlc}/CookedPCConsole/Mount.dlc")
        };
        let mapping = files
            .iter()
            .find(|file| file.destination.eq_ignore_ascii_case(&mount))
            .context("M3GS DLC is missing its mount file")?;
        let source = |path: &str| {
            sources
                .iter()
                .find(|source| source.relative == path)
                .context("Missing inspected M3GS input")
        };
        if game == Target::Le1 {
            super::m3da::mount(&super::m3da::read_text(root, source(&mapping.source)?)?)?;
        } else {
            super::m3to::mount(root, source(&mapping.source)?, game)?;
        }
        let source = source(&file.source)?;
        bytecode(&super::m3za::read_input(root, source, LIMIT as usize)?)?;
        plans.push(Plan {
            source: source.clone(),
            destination: file.destination.clone(),
            dlc: dlc.into(),
            index,
        });
    }
    Ok(plans)
}

pub(super) fn contributions(items: &[ShaderContribution]) -> Result<()> {
    ensure!(items.len() <= 4096, "Too many M3GS contributions");
    let mut mounts = BTreeMap::new();
    let mut dlcs = BTreeMap::new();
    let mut indices = BTreeSet::new();
    for item in items {
        super::manifest::relative_path(&item.shader.path)?;
        let (dlc, index) = location(&item.shader.path)?;
        let folded = dlc.to_ascii_lowercase();
        ensure!(
            dlc.eq_ignore_ascii_case(&item.dlc)
                && item.mount >= 0
                && index == item.index
                && (32..=LIMIT).contains(&item.shader.size),
            "Invalid M3GS contribution identity"
        );
        ensure!(
            indices.insert((folded.clone(), index)),
            "Multiple M3GS files replace shader {index} within DLC '{dlc}'"
        );
        ensure!(
            mounts
                .insert(item.mount, folded.clone())
                .is_none_or(|owner| owner == folded)
                && dlcs
                    .insert(folded, item.mount)
                    .is_none_or(|mount| mount == item.mount),
            "Ambiguous M3GS DLC mount order"
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::super::helper::protocol::TargetPackage;
    use super::super::helper::{FileIdentity, Job};
    use super::*;

    pub(in crate::core::game::mass_effect) fn dxbc(marker: u8) -> Vec<u8> {
        let mut data = vec![0; 48];
        data[..4].copy_from_slice(b"DXBC");
        for (offset, value) in [(20, 1u32), (24, 48), (28, 1), (32, 36), (40, 4)] {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        data[36..40].copy_from_slice(b"SHDR");
        data[44] = marker;
        data
    }

    // @variants: both
    #[test]
    fn rejects_truncated_overlapping_and_mislabelled_shader_payloads() -> Result<()> {
        let data = dxbc(1);
        bytecode(&data)?;
        for length in 0..data.len() {
            assert!(bytecode(&data[..length]).is_err());
        }
        for (offset, value) in [(0, 0), (20, 2), (24, 47), (28, u32::MAX), (32, 32), (40, 5)] {
            let mut invalid = data.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(bytecode(&invalid).is_err());
        }
        let mut overlap = vec![0; 52];
        overlap[..32].copy_from_slice(&data[..32]);
        for (offset, value) in [(24, 52u32), (28, 2), (32, 40), (36, 40), (44, 4)] {
            overlap[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        assert!(bytecode(&overlap).is_err());
        for path in [
            "../system/GlobalShader-0-x.m3gs",
            "~docs~/GlobalShader-0-x.m3gs",
            "Mods/GlobalShader-0-x.m3gs",
            "CookedPCConsole/GlobalShader-0-x.m3gs",
            "DLC/DLC_OFFICIAL/CookedPCConsole/GlobalShader-0-x.m3gs",
            "DLC/DLC_MOD_A/CookedPCConsole/GlobalShader--1-x.m3gs",
            "DLC/DLC_MOD_A/CookedPCConsole/GlobalShader-2147483648-x.m3gs",
        ] {
            assert!(location(path).is_err(), "{path}");
        }
        assert_eq!(
            location("DLC/DLC_MOD_A/CookedPCConsole/GlobalShader-001-x.m3gs")?.1,
            1
        );
        Ok(())
    }

    fn contribution(dlc: &str, mount: i32, index: u32) -> ShaderContribution {
        ShaderContribution {
            dlc: dlc.into(),
            mount,
            index,
            shader: FileIdentity {
                path: format!("DLC/{dlc}/CookedPCConsole/GlobalShader-{index}-x.m3gs"),
                size: 48,
                sha256: "0".repeat(64),
            },
        }
    }

    // @variants: both
    #[test]
    fn accepts_cross_dlc_replacements_but_rejects_ambiguous_shader_order() -> Result<()> {
        let first = contribution("DLC_MOD_A", 2, 0);
        let second = contribution("DLC_MOD_B", 1, 0);
        contributions(&[first.clone(), second.clone()])?;
        let mut duplicate = first.clone();
        duplicate.shader.path = duplicate.shader.path.replace("-0-x", "-00-other");
        assert!(contributions(&[first.clone(), duplicate]).is_err());
        assert!(contributions(&[first.clone(), contribution("DLC_MOD_B", 2, 1)]).is_err());
        assert!(contributions(&[first.clone(), contribution("DLC_MOD_A", 3, 1)]).is_err());
        for game in [Target::Le1, Target::Le2, Target::Le3] {
            for path in [
                TARGET,
                "CookedPCConsole/Engine.pcc",
                "../system/cache.bin",
                "~docs~/cache.bin",
                "Mods/cache.bin",
            ] {
                let input = FileIdentity {
                    path: path.into(),
                    size: 100,
                    sha256: "0".repeat(64),
                };
                let job = Job::Shaders {
                    game,
                    target: TargetPackage {
                        original: input.clone(),
                        current: input,
                    },
                    contributions: vec![first.clone(), second.clone()],
                };
                assert_eq!(job.validate().is_ok(), path == TARGET);
            }
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn inspection_requires_feature_level_mount_and_unchanged_sources() -> Result<()> {
        let root = tempdir()?;
        let mut sources = Vec::new();
        let mut files = Vec::new();
        for (name, bytes) in [
            ("AutoLoad.ini", b"[ME1DLCMOUNT]\nModMount=5\n".to_vec()),
            ("CookedPCConsole/GlobalShader-0-x.m3gs", dxbc(1)),
        ] {
            let path = format!("DLC/DLC_MOD_A/{name}");
            let destination = root.path().join(&path);
            std::fs::create_dir_all(destination.parent().context("Missing fixture parent")?)?;
            std::fs::write(destination, &bytes)?;
            sources.push(SourceFile {
                relative: path.clone(),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            });
            files.push(FileMapping {
                source: path.clone(),
                destination: path,
            });
        }
        assert_eq!(
            inspect(root.path(), &sources, &files, Target::Le1, "9.2")?.len(),
            1
        );
        for format in ["9.1", "10.0"] {
            assert!(inspect(root.path(), &sources, &files, Target::Le1, format).is_err());
        }
        for game in [Target::Le2, Target::Le3] {
            assert!(inspect(root.path(), &sources, &files, game, "9.2").is_err());
        }
        assert!(inspect(root.path(), &sources, &files[1..], Target::Le1, "9.2").is_err());
        let mut duplicate = files.clone();
        let mut extra = files[1].clone();
        extra.destination = extra.destination.replace("-0-x", "-00-other");
        duplicate.push(extra);
        assert!(inspect(root.path(), &sources, &duplicate, Target::Le1, "9.2").is_err());
        std::fs::write(root.path().join(&sources[1].relative), dxbc(2))?;
        assert!(inspect(root.path(), &sources, &files, Target::Le1, "9.2").is_err());
        Ok(())
    }
}
