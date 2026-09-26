use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use lzma_rust2::LzmaReader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Target;
use super::manifest::relative_path;
use super::package::SourceFile;

const MAX_CONTAINER: usize = 512 * 1024 * 1024;
const MAX_MANIFEST: usize = 4 * 1024 * 1024;
const MAX_ASSET: usize = 128 * 1024 * 1024;
const MAX_DICTIONARY: u32 = 16 * 1024 * 1024;
pub(super) fn targets(game: Target) -> &'static [&'static str] {
    match game {
        Target::Le1 => &[
            "Core.pcc",
            "Engine.pcc",
            "IpDrv.pcc",
            "GFxUI.pcc",
            "PlotManagerMap.pcc",
            "PlotManagerMap_LOC_INT.pcc",
            "SFXOnlineFoundation.pcc",
            "SFXGame.pcc",
            "SFXStrategicAI.pcc",
            "SFXGameContent_Powers.pcc",
            "PlotManager.pcc",
            "PlotManagerDLC_UNC.pcc",
            "BIOC_Materials.pcc",
            "SFXWorldResources.pcc",
            "SFXVehicleResources.pcc",
            "Startup_DE.pcc",
            "Startup_ES.pcc",
            "Startup_FE.pcc",
            "Startup_FR.pcc",
            "Startup_GE.pcc",
            "Startup_IE.pcc",
            "Startup_INT.pcc",
            "Startup_IT.pcc",
            "Startup_JA.pcc",
            "Startup_PL.pcc",
            "Startup_PLPC.pcc",
            "Startup_RA.pcc",
            "Startup_RU.pcc",
            "EntryMenu.pcc",
            "EntryMenu_LOC_DE.pcc",
            "EntryMenu_LOC_FR.pcc",
            "EntryMenu_LOC_INT.pcc",
            "EntryMenu_LOC_IT.pcc",
            "EntryMenu_LOC_PLPC.pcc",
            "EntryMenu_LOC_RA.pcc",
        ],
        Target::Le2 => &[
            "Core.pcc",
            "Engine.pcc",
            "IpDrv.pcc",
            "GFxUI.pcc",
            "WwiseAudio.pcc",
            "SFXOnlineFoundation.pcc",
            "PlotManagerMap.pcc",
            "PlotManagerMap_LOC_INT.pcc",
            "SFXGame.pcc",
            "Startup_DEU.pcc",
            "Startup_ESN.pcc",
            "Startup_FRA.pcc",
            "Startup_INT.pcc",
            "Startup_ITA.pcc",
            "Startup_JPN.pcc",
            "Startup_POL.pcc",
            "Startup_RUS.pcc",
            "EntryMenu.pcc",
            "EntryMenu_LOC_DEU.pcc",
            "EntryMenu_LOC_FRA.pcc",
            "EntryMenu_LOC_INT.pcc",
            "EntryMenu_LOC_ITA.pcc",
            "EntryMenu_LOC_POL.pcc",
        ],
        Target::Le3 => &[
            "Core.pcc",
            "Engine.pcc",
            "GameFramework.pcc",
            "IpDrv.pcc",
            "GFxUI.pcc",
            "WwiseAudio.pcc",
            "SFXOnlineFoundation.pcc",
            "SFXGame.pcc",
            "Startup.pcc",
            "EntryMenu.pcc",
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct M3mPlan {
    pub(crate) game: Target,
    pub(crate) source: SourceFile,
    pub(crate) version: u8,
    pub(crate) files: Vec<PackageMerge>,
    pub(crate) assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) offset: u64,
    pub(crate) stored_size: u64,
    pub(crate) size: u64,
    pub(crate) compressed: bool,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackageMerge {
    pub(crate) target: String,
    pub(crate) all_localizations: bool,
    pub(crate) target_candidates: Vec<String>,
    pub(crate) changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Change {
    pub(crate) entry: String,
    pub(crate) operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    Asset {
        asset: String,
        entry: String,
        allow_new: bool,
    },
    Class {
        script: Script,
    },
    Function {
        script: Script,
    },
    Members {
        scripts: Vec<Script>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Script {
    Inline { name: String, text: String },
    Asset { name: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    game: Target,
    files: Vec<MergeFile>,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeFile {
    filename: String,
    #[serde(default)]
    applytoalllocalizations: bool,
    changes: Vec<MergeChange>,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MergeChange {
    entryname: String,
    assetupdate: Option<AssetUpdate>,
    classupdate: Option<ClassUpdate>,
    scriptupdate: Option<ScriptUpdate>,
    addtoclassorreplace: Option<MembersUpdate>,
    propertyupdates: Option<serde::de::IgnoredAny>,
    sequenceskipupdate: Option<serde::de::IgnoredAny>,
    newassetupdate: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    disableconfigupdate: bool,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssetUpdate {
    assetname: String,
    entryname: String,
    #[serde(default)]
    canmergeasnew: bool,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassUpdate {
    assetname: String,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptUpdate {
    scriptfilename: String,
    scripttext: Option<String>,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MembersUpdate {
    scriptfilenames: Vec<String>,
    scripts: Option<Vec<Option<String>>>,
    #[serde(rename = "comment")]
    _comment: Option<String>,
}

pub(super) fn inspect(root: &Path, source: &SourceFile, target: Target) -> Result<M3mPlan> {
    parse(&read_source(root, source)?, source.clone(), target)
        .with_context(|| format!("Invalid M3M '{}'", source.relative))
}

fn read_source(root: &Path, source: &SourceFile) -> Result<Vec<u8>> {
    ensure!(
        source.size <= MAX_CONTAINER as u64,
        "M3M exceeds its container size limit"
    );
    let path = root.join(relative_path(&source.relative)?);
    ensure!(
        path.is_file() && !path.is_symlink(),
        "M3M must be a regular file"
    );
    let mut data = Vec::new();
    File::open(path)?
        .take(source.size + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 == source.size && digest(&data) == source.sha256,
        "M3M source changed after inspection"
    );
    Ok(data)
}

pub(super) fn materialize(root: &Path, plan: &M3mPlan) -> Result<BTreeMap<String, Vec<u8>>> {
    let data = read_source(root, &plan.source)?;
    ensure!(
        parse(&data, plan.source.clone(), plan.game)? == *plan,
        "M3M plan changed after inspection"
    );
    let mut assets = BTreeMap::new();
    for asset in &plan.assets {
        let start = usize::try_from(asset.offset)?;
        let end = start
            .checked_add(usize::try_from(asset.stored_size)?)
            .context("M3M asset range overflow")?;
        let stored = data
            .get(start..end)
            .context("M3M asset lies outside its source")?;
        let bytes = if asset.compressed {
            decompress(stored, usize::try_from(asset.size)?)?
        } else {
            stored.to_vec()
        };
        ensure!(
            bytes.len() as u64 == asset.size && digest(&bytes) == asset.sha256,
            "M3M asset changed after inspection"
        );
        assets.insert(asset.name.clone(), bytes);
    }
    Ok(assets)
}

fn parse(data: &[u8], source: SourceFile, target: Target) -> Result<M3mPlan> {
    ensure!(
        data.len() <= MAX_CONTAINER,
        "M3M exceeds its container size limit"
    );
    let mut reader = Cursor::new(data);
    ensure!(bytes(&mut reader, 4)? == b"M3MM", "Invalid M3M magic");
    let version = byte(&mut reader)?;
    ensure!(matches!(version, 1 | 2), "Unsupported M3M version");
    let text = if version == 1 {
        unreal_string(&mut reader, MAX_MANIFEST)?
    } else {
        let size = length(&mut reader, MAX_MANIFEST)?;
        let stored = length(&mut reader, MAX_MANIFEST)?;
        let decoded = decompress(bytes(&mut reader, stored)?, size)?;
        let mut manifest = Cursor::new(decoded.as_slice());
        let text = unreal_string(&mut manifest, MAX_MANIFEST)?;
        ensure!(
            manifest.position() == size as u64,
            "Trailing data in M3M manifest"
        );
        text
    };
    let manifest: Manifest =
        serde_json::from_str(&text).context("Invalid or unsupported M3M manifest")?;
    ensure!(
        manifest.game == target,
        "M3M target differs from moddesc.ini"
    );
    ensure!(
        !manifest.files.is_empty() && manifest.files.len() <= 1024,
        "Invalid M3M file count"
    );
    let count = length(&mut reader, 4096)?;
    let mut assets = Vec::new();
    let mut names = BTreeSet::new();
    let mut total = 0;
    for _ in 0..count {
        ensure!(bytes(&mut reader, 4)? == b"MMV1", "Invalid M3M asset magic");
        let name = unreal_string(&mut reader, 1024)?;
        filename(&name, &["pcc", "uc"])?;
        ensure!(
            names.insert(name.to_ascii_lowercase()),
            "Duplicate or case-colliding M3M asset"
        );
        let size = length(&mut reader, MAX_ASSET)?;
        ensure!(size > 0, "Empty M3M asset");
        total += size;
        ensure!(
            total <= MAX_CONTAINER,
            "Decoded M3M assets exceed the size limit"
        );
        let flag = if version == 2 { byte(&mut reader)? } else { 0 };
        ensure!(flag <= 1, "Invalid M3M compression flag");
        let compressed = flag == 1;
        let stored_size = if compressed {
            length(&mut reader, MAX_ASSET)?
        } else {
            size
        };
        let offset = reader.position();
        let raw = bytes(&mut reader, stored_size)?;
        let decoded;
        let content = if compressed {
            decoded = decompress(raw, size)?;
            &decoded
        } else {
            raw
        };
        if name.to_ascii_lowercase().ends_with(".uc") {
            script_text(std::str::from_utf8(content).context("M3M script is not UTF-8")?)?;
        } else {
            ensure!(
                content.starts_with(&[0xc1, 0x83, 0x2a, 0x9e]),
                "M3M asset is not an Unreal package"
            );
        }
        assets.push(Asset {
            name,
            offset,
            stored_size: stored_size as u64,
            size: size as u64,
            compressed,
            sha256: digest(content),
        });
    }
    ensure!(
        reader.position() == data.len() as u64,
        "Trailing data in M3M container"
    );
    let index = assets
        .iter()
        .map(|asset| (asset.name.to_ascii_lowercase(), asset))
        .collect();
    let mut files = Vec::new();
    let mut change_count = 0;
    for file in manifest.files {
        let target_candidates =
            target_candidates(target, &file.filename, file.applytoalllocalizations)?;
        ensure!(!file.changes.is_empty(), "Empty M3M target changes");
        change_count += file.changes.len();
        ensure!(change_count <= 4096, "M3M exceeds its change count limit");
        let changes = file
            .changes
            .into_iter()
            .map(|change| normalize(change, version, &index))
            .collect::<Result<Vec<_>>>()?;
        files.push(PackageMerge {
            target: file.filename,
            all_localizations: file.applytoalllocalizations,
            target_candidates,
            changes,
        });
    }
    Ok(M3mPlan {
        game: target,
        source,
        version,
        files,
        assets,
    })
}

fn target_candidates(game: Target, name: &str, all_localizations: bool) -> Result<Vec<String>> {
    let targets = targets(game);
    let target = targets
        .iter()
        .find(|target| target.eq_ignore_ascii_case(name))
        .with_context(|| format!("Unsupported M3M target '{name}'"))?;
    if !all_localizations {
        return Ok(vec![(*target).to_owned()]);
    }
    let prefix = if target.starts_with("Startup_") {
        "Startup_"
    } else if target.starts_with("EntryMenu_LOC_") {
        "EntryMenu_LOC_"
    } else if target.starts_with("PlotManagerMap_LOC_") {
        "PlotManagerMap_LOC_"
    } else {
        anyhow::bail!("Unsupported localized M3M target");
    };
    Ok(targets
        .iter()
        .filter(|target| target.starts_with(prefix))
        .map(|target| (*target).to_owned())
        .collect())
}

fn normalize(
    change: MergeChange,
    version: u8,
    assets: &BTreeMap<String, &Asset>,
) -> Result<Change> {
    entry_name(&change.entryname)?;
    ensure!(
        change.propertyupdates.is_none()
            && change.sequenceskipupdate.is_none()
            && change.newassetupdate.is_none()
            && !change.disableconfigupdate,
        "M3M contains an unsupported update operation"
    );
    let mut operations = Vec::new();
    if let Some(update) = change.assetupdate {
        entry_name(&update.entryname)?;
        let asset = asset_name(assets, &update.assetname, "pcc")?;
        operations.push(Operation::Asset {
            asset,
            entry: update.entryname,
            allow_new: update.canmergeasnew,
        });
    }
    if let Some(update) = change.classupdate {
        ensure!(
            version == 2 && change.entryname.split('.').count() <= 2,
            "Class updates require M3M v2 and at most one containing package"
        );
        let name = asset_name(assets, &update.assetname, "uc")?;
        operations.push(Operation::Class {
            script: Script::Asset { name },
        });
    }
    if let Some(update) = change.scriptupdate {
        let script = script(version, update.scriptfilename, update.scripttext, assets)?;
        operations.push(Operation::Function { script });
    }
    if let Some(update) = change.addtoclassorreplace {
        let count = update.scriptfilenames.len();
        ensure!(
            (1..=1024).contains(&count),
            "Invalid M3M class member count"
        );
        let texts = update.scripts.unwrap_or_else(|| vec![None; count]);
        ensure!(texts.len() == count, "Mismatched M3M class member lists");
        let mut names = BTreeSet::new();
        let scripts = update
            .scriptfilenames
            .into_iter()
            .zip(texts)
            .map(|(name, text)| {
                ensure!(
                    names.insert(name.to_ascii_lowercase()),
                    "Duplicate M3M class member script"
                );
                script(version, name, text, assets)
            })
            .collect::<Result<_>>()?;
        operations.push(Operation::Members { scripts });
    }
    ensure!(
        !operations.is_empty(),
        "M3M change has no supported operations"
    );
    Ok(Change {
        entry: change.entryname,
        operations,
    })
}

fn script(
    version: u8,
    name: String,
    text: Option<String>,
    assets: &BTreeMap<String, &Asset>,
) -> Result<Script> {
    filename(&name, &["uc"])?;
    if version == 1 {
        let text = text.context("M3M v1 requires inline script text")?;
        script_text(&text)?;
        Ok(Script::Inline { name, text })
    } else {
        ensure!(text.is_none(), "M3M v2 cannot contain inline script text");
        Ok(Script::Asset {
            name: asset_name(assets, &name, "uc")?,
        })
    }
}

fn asset_name(assets: &BTreeMap<String, &Asset>, name: &str, extension: &str) -> Result<String> {
    filename(name, &[extension])?;
    assets
        .get(&name.to_ascii_lowercase())
        .map(|asset| asset.name.clone())
        .with_context(|| format!("Missing M3M asset '{name}'"))
}

fn filename(name: &str, extensions: &[&str]) -> Result<()> {
    ensure!(
        name.len() <= 255
            && relative_path(name)? == name
            && !name.contains('/')
            && name
                .rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty()
                    && extensions
                        .iter()
                        .any(|allowed| ext.eq_ignore_ascii_case(allowed))),
        "Invalid M3M asset filename"
    );
    Ok(())
}

fn entry_name(name: &str) -> Result<()> {
    ensure!(
        name.len() <= 1024
            && name.split('.').all(|part| !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')),
        "Invalid M3M export path"
    );
    Ok(())
}

fn script_text(text: &str) -> Result<()> {
    ensure!(
        !text.trim_start_matches('\u{feff}').trim().is_empty()
            && text.len() <= MAX_MANIFEST
            && !text.contains('\0'),
        "Invalid or oversized M3M UnrealScript source"
    );
    Ok(())
}

fn digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn bytes<'a>(reader: &mut Cursor<&'a [u8]>, count: usize) -> Result<&'a [u8]> {
    let start = usize::try_from(reader.position())?;
    let end = start.checked_add(count).context("M3M size overflow")?;
    let data = reader
        .get_ref()
        .get(start..end)
        .context("Truncated M3M data")?;
    reader.set_position(end as u64);
    Ok(data)
}

fn byte(reader: &mut Cursor<&[u8]>) -> Result<u8> {
    Ok(bytes(reader, 1)?[0])
}

fn integer(reader: &mut Cursor<&[u8]>) -> Result<i32> {
    Ok(i32::from_le_bytes(bytes(reader, 4)?.try_into()?))
}

fn length(reader: &mut Cursor<&[u8]>, limit: usize) -> Result<usize> {
    let size = usize::try_from(integer(reader)?).context("Negative M3M size")?;
    ensure!(size <= limit, "M3M field exceeds its size limit");
    Ok(size)
}

fn unreal_string(reader: &mut Cursor<&[u8]>, limit: usize) -> Result<String> {
    let count = integer(reader)?;
    ensure!(count != 0 && count != i32::MIN, "Invalid M3M string size");
    let size = count.unsigned_abs() as usize;
    ensure!(
        size <= limit / if count < 0 { 2 } else { 1 },
        "M3M string exceeds its size limit"
    );
    let text = if count < 0 {
        let words: Vec<u16> = bytes(reader, size * 2)?
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        ensure!(words.last() == Some(&0), "Unterminated M3M string");
        String::from_utf16(&words[..words.len() - 1]).context("Invalid M3M UTF-16 string")?
    } else {
        let data = bytes(reader, size)?;
        ensure!(
            data.last() == Some(&0) && data.is_ascii(),
            "Invalid M3M ASCII string"
        );
        String::from_utf8(data[..data.len() - 1].to_vec())?
    };
    ensure!(!text.contains('\0'), "Embedded null in M3M string");
    Ok(text)
}

struct CompressedInput<'a> {
    data: &'a [u8],
    exhausted: bool,
}

impl Read for CompressedInput<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        // The codec substitutes zero on EOF; record it so truncated streams cannot pass.
        self.exhausted |= !buffer.is_empty() && self.data.is_empty();
        self.data.read(buffer)
    }
}

fn decompress(data: &[u8], size: usize) -> Result<Vec<u8>> {
    decompress_padded(data, size, 0)
}

pub(super) fn decompress_padded(data: &[u8], size: usize, padding: usize) -> Result<Vec<u8>> {
    ensure!(data.len() >= 10 && size > 0, "Truncated merge LZMA stream");
    let dictionary = u32::from_le_bytes(data[1..5].try_into()?);
    ensure!(
        dictionary <= MAX_DICTIONARY,
        "Merge LZMA dictionary exceeds its size limit"
    );
    // The pinned compressor writes an end marker; require it to validate the declared size.
    let input = CompressedInput {
        data: &data[5..],
        exhausted: false,
    };
    let mut reader = LzmaReader::new_with_props(input, u64::MAX, data[0], dictionary, None)
        .context("Invalid merge LZMA properties")?;
    let mut decoded = Vec::new();
    reader
        .by_ref()
        .take(size as u64 + padding as u64 + 1)
        .read_to_end(&mut decoded)
        .context("Invalid merge LZMA data or end marker")?;
    let input = reader.into_inner();
    ensure!(
        decoded.len() >= size
            && decoded.len() <= size + padding
            && decoded[size..].iter().all(|byte| *byte == 0)
            && input.data.is_empty()
            && !input.exhausted,
        "Merge LZMA size mismatch or trailing data"
    );
    decoded.truncate(size);
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use lzma_rust2::{LzmaOptions, LzmaWriter};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn manifest() -> serde_json::Value {
        json!({"game":"LE1", "files":[{"filename":"SFXGame.pcc", "changes":[{
            "entryname":"Example.Fn", "scriptupdate":{
                "scriptfilename":"Example.uc", "scripttext":"function Fn() {}"
            }
        }]}]})
    }

    fn string(data: &mut Vec<u8>, text: &str) {
        data.extend_from_slice(&(text.len() as i32 + 1).to_le_bytes());
        data.extend_from_slice(text.as_bytes());
        data.push(0);
    }

    fn container(text: &str, assets: &[(&str, &[u8])]) -> Vec<u8> {
        let mut data = b"M3MM\x01".to_vec();
        string(&mut data, text);
        data.extend_from_slice(&(assets.len() as i32).to_le_bytes());
        for (name, content) in assets {
            data.extend_from_slice(b"MMV1");
            string(&mut data, name);
            data.extend_from_slice(&(content.len() as i32).to_le_bytes());
            data.extend_from_slice(content);
        }
        data
    }

    fn inspect_bytes(data: &[u8]) -> Result<M3mPlan> {
        parse(
            data,
            SourceFile {
                relative: "MergeMods/Example.m3m".into(),
                size: data.len() as u64,
                sha256: digest(data),
            },
            Target::Le1,
        )
    }

    // @variants: both
    #[test]
    fn accepts_game_specific_startup_and_localized_asset_targets() -> Result<()> {
        for (game, names) in [
            (
                Target::Le1,
                vec!["PlotManagerMap_LOC_INT.pcc", "EntryMenu_LOC_PLPC.pcc"],
            ),
            (
                Target::Le2,
                vec![
                    "Startup_DEU.pcc",
                    "Startup_ESN.pcc",
                    "Startup_FRA.pcc",
                    "Startup_ITA.pcc",
                    "Startup_JPN.pcc",
                    "Startup_POL.pcc",
                    "Startup_RUS.pcc",
                    "PlotManagerMap_LOC_INT.pcc",
                    "EntryMenu_LOC_FRA.pcc",
                ],
            ),
            (Target::Le3, vec!["Startup.pcc"]),
        ] {
            for name in names {
                let value = json!({"game": game, "files": [{"filename": name.to_lowercase(),
                "changes": [{"entryname": "GUI_SF_Options.Options", "assetupdate": {
                    "assetname": "OptionsScalingFix.pcc", "entryname": "GUI_SF_Options.Options"
                }}]}]});
                for data in [
                    container(
                        &value.to_string(),
                        &[("OptionsScalingFix.pcc", &[0xc1, 0x83, 0x2a, 0x9e])],
                    ),
                    v2(
                        &value.to_string(),
                        &[("OptionsScalingFix.pcc", &[0xc1, 0x83, 0x2a, 0x9e], true)],
                    )?,
                ] {
                    let source = SourceFile {
                        relative: "Options.m3m".into(),
                        size: data.len() as u64,
                        sha256: digest(&data),
                    };
                    let plan = parse(&data, source, game)?;
                    assert_eq!(plan.files[0].target_candidates, [name]);
                    assert!(matches!(
                        plan.files[0].changes[0].operations[0],
                        Operation::Asset { .. }
                    ));
                }
            }
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn expands_only_the_selected_games_localization_family() -> Result<()> {
        assert_eq!(
            target_candidates(Target::Le2, "startup_fra.PCC", true)?,
            ["DEU", "ESN", "FRA", "INT", "ITA", "JPN", "POL", "RUS"]
                .map(|code| format!("Startup_{code}.pcc"))
        );
        assert_eq!(
            target_candidates(Target::Le1, "EntryMenu_LOC_INT.pcc", true)?,
            ["DE", "FR", "INT", "IT", "PLPC", "RA"].map(|code| format!("EntryMenu_LOC_{code}.pcc"))
        );
        assert_eq!(
            target_candidates(Target::Le2, "EntryMenu_LOC_INT.pcc", true)?,
            ["DEU", "FRA", "INT", "ITA", "POL"].map(|code| format!("EntryMenu_LOC_{code}.pcc"))
        );
        assert_eq!(
            target_candidates(Target::Le1, "PlotManagerMap_LOC_INT.pcc", true)?,
            ["PlotManagerMap_LOC_INT.pcc"]
        );
        assert!(target_candidates(Target::Le3, "Startup.pcc", true).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_other_games_targets_and_noncanonical_package_names() {
        for (game, names) in [
            (
                Target::Le1,
                vec![
                    "Startup.pcc",
                    "Startup_FRA.pcc",
                    "EntryMenu_LOC_DEU.pcc",
                    "GameFramework.pcc",
                ],
            ),
            (
                Target::Le2,
                vec![
                    "Startup.pcc",
                    "Startup_DE.pcc",
                    "EntryMenu_LOC_PLPC.pcc",
                    "BIOC_Materials.pcc",
                ],
            ),
            (
                Target::Le3,
                vec![
                    "Startup_INT.pcc",
                    "EntryMenu_LOC_INT.pcc",
                    "PlotManagerMap.pcc",
                ],
            ),
        ] {
            for name in names.into_iter().chain([
                "EngineTest.pcc",
                "Startup_MOD_TEST_INT.pcc",
                "Startup.pcc.extra",
                "../Startup.pcc",
                "../system/Startup.pcc",
                "~docs~/Startup.pcc",
                "Mods/Startup.pcc",
                "CookedPCConsole/Startup.pcc",
            ]) {
                for localized in [false, true] {
                    assert!(
                        target_candidates(game, name, localized).is_err(),
                        "{game:?}: {name}"
                    );
                }
            }
        }
    }

    // @variants: both
    #[test]
    fn accepts_le2_and_le3_assets_and_scripts_but_rejects_wrong_games() -> Result<()> {
        for (game, name) in [(Target::Le2, "LE2"), (Target::Le3, "LE3")] {
            let manifest = json!({"game": name, "files": [{"filename":"SFXGame.pcc", "changes":[{
                "entryname":"Example.Asset", "assetupdate":{"assetname":"Asset.pcc", "entryname":"Example.Asset"}
            }]}]});
            let data = container(
                &manifest.to_string(),
                &[("Asset.pcc", &[0xc1, 0x83, 0x2a, 0x9e])],
            );
            let source = SourceFile {
                relative: "Example.m3m".into(),
                size: data.len() as u64,
                sha256: digest(&data),
            };
            let plan = parse(&data, source.clone(), game)?;
            assert_eq!(plan.game, game);
            assert!(parse(&data, source, Target::Le1).is_err());
            let root = tempdir()?;
            fs::write(root.path().join("Example.m3m"), &data)?;
            assert_eq!(
                materialize(root.path(), &plan)?["Asset.pcc"],
                [0xc1, 0x83, 0x2a, 0x9e]
            );
            let mut forged = plan;
            forged.game = Target::Le1;
            assert!(materialize(root.path(), &forged).is_err());
            let mut scripts = self::manifest();
            scripts["game"] = name.into();
            let data = container(&scripts.to_string(), &[]);
            let source = SourceFile {
                relative: "Example.m3m".into(),
                size: data.len() as u64,
                sha256: digest(&data),
            };
            assert_eq!(parse(&data, source, game)?.game, game);
        }
        Ok(())
    }

    fn compressed(data: &[u8]) -> Result<Vec<u8>> {
        let options = LzmaOptions {
            dict_size: 65536,
            ..LzmaOptions::default()
        };
        let mut writer = LzmaWriter::new_no_header(Vec::new(), &options, true)?;
        let mut output = vec![writer.props()];
        output.extend_from_slice(&options.dict_size.to_le_bytes());
        writer.write_all(data)?;
        output.extend_from_slice(&writer.finish()?);
        Ok(output)
    }

    fn v2(text: &str, assets: &[(&str, &[u8], bool)]) -> Result<Vec<u8>> {
        let mut manifest = Vec::new();
        let words: Vec<_> = text.encode_utf16().chain([0]).collect();
        manifest.extend_from_slice(&(-(words.len() as i32)).to_le_bytes());
        for word in words {
            manifest.extend_from_slice(&word.to_le_bytes());
        }
        let stored = compressed(&manifest)?;
        let mut data = b"M3MM\x02".to_vec();
        data.extend_from_slice(&(manifest.len() as i32).to_le_bytes());
        data.extend_from_slice(&(stored.len() as i32).to_le_bytes());
        data.extend_from_slice(&stored);
        data.extend_from_slice(&(assets.len() as i32).to_le_bytes());
        for (name, content, compress) in assets {
            data.extend_from_slice(b"MMV1");
            string(&mut data, name);
            data.extend_from_slice(&(content.len() as i32).to_le_bytes());
            data.push(u8::from(*compress));
            if *compress {
                let stored = compressed(content)?;
                data.extend_from_slice(&(stored.len() as i32).to_le_bytes());
                data.extend_from_slice(&stored);
            } else {
                data.extend_from_slice(content);
            }
        }
        Ok(data)
    }

    // @variants: both
    #[test]
    fn materializes_verified_assets_and_rejects_changed_plans_or_sources() -> Result<()> {
        let text = "function Fn() {}";
        let mut value = manifest();
        let first = container(&value.to_string(), &[("Example.uc", text.as_bytes())]);
        value["files"][0]["changes"][0]["scriptupdate"]["scripttext"] = json!(null);
        for bytes in [
            first,
            v2(&value.to_string(), &[("Example.uc", text.as_bytes(), true)])?,
        ] {
            let root = tempdir()?;
            let plan = inspect_bytes(&bytes)?;
            fs::create_dir(root.path().join("MergeMods"))?;
            let path = root.path().join(&plan.source.relative);
            fs::write(&path, &bytes)?;
            assert_eq!(
                materialize(root.path(), &plan)?["Example.uc"],
                text.as_bytes()
            );
            let mut changed = plan.clone();
            changed.assets[0].offset += 1;
            assert!(materialize(root.path(), &changed).is_err());
            let mut changed = plan.clone();
            changed.files[0].changes[0].entry = "Other.Fn".into();
            assert!(materialize(root.path(), &changed).is_err());
            fs::write(&path, b"changed source")?;
            assert!(materialize(root.path(), &plan).is_err());
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn decodes_v2_unicode_manifests_and_compressed_or_raw_assets() -> Result<()> {
        let mut value = manifest();
        value["comment"] = json!("Édition française 日本語");
        value["files"][0]["changes"][0]["scriptupdate"]["scripttext"] = json!(null);
        for compress in [false, true] {
            let bytes = v2(
                &value.to_string(),
                &[("Example.uc", b"function Fn() {}", compress)],
            )?;
            let plan = inspect_bytes(&bytes)?;
            assert_eq!(plan.version, 2);
            assert_eq!(plan.assets[0].compressed, compress);
            assert_eq!(plan.assets[0].sha256, digest(b"function Fn() {}"));
            assert!(
                matches!(&plan.files[0].changes[0].operations[0], Operation::Function {
                script: Script::Asset { name }} if name == "Example.uc")
            );
            for size in 0..bytes.len() {
                assert!(inspect_bytes(&bytes[..size]).is_err());
            }
            let mut invalid = bytes;
            let flag = plan.assets[0].offset as usize - if compress { 5 } else { 1 };
            invalid[flag] = 2;
            assert!(inspect_bytes(&invalid).is_err());
        }
        Ok(())
    }

    #[test]
    fn rejects_lzma_corruption_overruns_and_excessive_dictionaries() -> Result<()> {
        let plain = b"function Fn() {}";
        let data = compressed(plain)?;
        assert_eq!(decompress(&data, plain.len())?, plain);
        assert!(decompress(&data, plain.len() - 1).is_err());
        assert!(decompress(&data, plain.len() + 1).is_err());
        for size in 0..data.len() {
            assert!(decompress(&data[..size], plain.len()).is_err());
        }
        let mut invalid = data.clone();
        invalid.push(0);
        assert!(decompress(&invalid, plain.len()).is_err());
        invalid = data.clone();
        invalid[0] = 255;
        assert!(decompress(&invalid, plain.len()).is_err());
        invalid = data;
        invalid[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decompress(&invalid, plain.len()).is_err());
        Ok(())
    }

    #[test]
    fn validates_lzma_end_markers_without_accepting_trailing_data() -> Result<()> {
        let data = [
            93, 0, 0, 1, 0, 0, 51, 29, 73, 255, 169, 97, 251, 19, 6, 154, 225, 163, 254, 253, 33,
            26, 65, 101, 32, 116, 255, 246, 165, 128, 0,
        ];
        assert_eq!(decompress(&data, 15)?, b"function F() {}");
        for size in [14, 16] {
            assert!(decompress(&data, size).is_err());
        }
        let mut extra = data.to_vec();
        extra.push(0);
        assert!(decompress(&extra, 15).is_err());
        for size in 0..data.len() {
            assert!(decompress(&data[..size], 15).is_err());
        }
        Ok(())
    }

    #[test]
    fn enforces_script_and_class_update_version_semantics() -> Result<()> {
        let value = manifest();
        assert!(
            inspect_bytes(&v2(&value.to_string(), &[("Example.uc", b"source", true)])?).is_err()
        );
        let mut value = value;
        value["files"][0]["changes"][0] =
            json!({"entryname":"CustomClass", "classupdate":{"assetname":"Example.uc"}});
        assert!(
            inspect_bytes(&container(&value.to_string(), &[("Example.uc", b"source")])).is_err()
        );
        let plan = inspect_bytes(&v2(
            &value.to_string(),
            &[("Example.uc", b"class CustomClass extends Object;", true)],
        )?)?;
        assert!(matches!(
            plan.files[0].changes[0].operations[0],
            Operation::Class { .. }
        ));
        value["files"][0]["changes"][0]["entryname"] = json!("Outer.Inner.Class");
        assert!(
            inspect_bytes(&v2(&value.to_string(), &[("Example.uc", b"source", true)])?).is_err()
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn retains_script_text_and_normalizes_operation_order() -> Result<()> {
        let mut value = manifest();
        let change = &mut value["files"][0]["changes"][0];
        change["addtoclassorreplace"] = json!({"scriptfilenames":["B.uc","A.uc"],
            "scripts":["function B() {}", "function A() {}"]});
        change["assetupdate"] =
            json!({"assetname":"example.PCC", "entryname":"Source.Fn", "canmergeasnew":true});
        let plan = inspect_bytes(&container(
            &value.to_string(),
            &[("Example.pcc", &[0xc1, 0x83, 0x2a, 0x9e])],
        ))?;
        let operations = &plan.files[0].changes[0].operations;
        assert!(
            matches!(&operations[0], Operation::Asset { asset, entry, allow_new:true }
            if asset == "Example.pcc" && entry == "Source.Fn")
        );
        assert!(
            matches!(&operations[1], Operation::Function { script:Script::Inline { text, .. } }
            if text == "function Fn() {}")
        );
        assert!(matches!(&operations[2], Operation::Members { scripts }
            if matches!(&scripts[0], Script::Inline { name, .. } if name == "B.uc")));
        let asset = &plan.assets[0];
        assert_eq!(asset.sha256, digest(&[0xc1, 0x83, 0x2a, 0x9e]));
        assert_eq!(asset.size, 4);
        assert!(!asset.compressed);
        Ok(())
    }

    #[test]
    fn rejects_unknown_duplicate_and_unsupported_manifest_semantics() {
        let valid = manifest().to_string();
        for text in [
            valid.replace("\"game\":\"LE1\"", "\"game\":\"LE1\",\"game\":\"LE1\""),
            valid.replace("\"game\":\"LE1\"", "\"game\":\"LE1\",\"future\":true"),
            valid.replace("\"entryname\":", "\"propertyupdates\":[],\"entryname\":"),
            valid.replace("\"entryname\":", "\"sequenceskipupdate\":{},\"entryname\":"),
            valid.replace(
                "\"entryname\":",
                "\"disableconfigupdate\":true,\"entryname\":",
            ),
            valid.replace("\"entryname\":", "\"newassetupdate\":{},\"entryname\":"),
            valid.replace("\"LE1\"", "\"LE2\""),
            valid.replace("\"LE1\"", "\"LE3\""),
            valid.replace("\"LE1\"", "\"ME1\""),
            valid.replace("SFXGame.pcc", "../Engine.pcc"),
            valid.replace("SFXGame.pcc", "Saves.pcc"),
            valid.replace("Example.Fn", "Example..Fn"),
        ] {
            assert!(
                inspect_bytes(&container(&text, &[])).is_err(),
                "accepted {text}"
            );
        }
    }

    #[test]
    fn rejects_truncation_trailing_bytes_and_invalid_container_sizes() {
        let valid = container(
            &manifest().to_string(),
            &[("Example.uc", b"function F() {}")],
        );
        for size in 0..valid.len() {
            assert!(
                inspect_bytes(&valid[..size]).is_err(),
                "accepted truncation at {size}"
            );
        }
        for version in [0, 3, 255] {
            let mut data = valid.clone();
            data[4] = version;
            assert!(inspect_bytes(&data).is_err());
        }
        for size in [i32::MIN, i32::MAX, 0] {
            let mut data = valid.clone();
            data[5..9].copy_from_slice(&size.to_le_bytes());
            assert!(inspect_bytes(&data).is_err());
        }
        let mut data = valid;
        data.push(0);
        assert!(inspect_bytes(&data).is_err());
    }

    #[test]
    fn rejects_missing_assets_unsafe_names_case_collisions_and_invalid_scripts() {
        let mut value = manifest();
        value["files"][0]["changes"][0]["assetupdate"] =
            json!({"assetname":"Missing.pcc", "entryname":"Example"});
        assert!(inspect_bytes(&container(&value.to_string(), &[])).is_err());
        let valid = manifest().to_string();
        for name in [
            "../file.uc",
            "nested/file.uc",
            "C:/file.uc",
            "file.dll",
            "file.headmorph",
            "file.uc ",
            "file\\name.uc",
        ] {
            assert!(inspect_bytes(&container(&valid, &[(name, b"function F() {}")])).is_err());
        }
        for data in [b"".as_slice(), b" ", b"function\0Fn", &[0xff]] {
            assert!(inspect_bytes(&container(&valid, &[("Example.uc", data)])).is_err());
        }
        assert!(inspect_bytes(&container(&valid, &[("Example.pcc", b"not a package")])).is_err());
        assert!(
            inspect_bytes(&container(
                &valid,
                &[("a.uc", b"source"), ("A.uc", b"source")]
            ))
            .is_err()
        );
    }

    #[test]
    fn rejects_mismatched_or_ambiguous_class_member_lists() {
        for update in [
            json!({"scriptfilenames":["A.uc"],"scripts":[]}),
            json!({"scriptfilenames":["A.uc"]}),
            json!({"scriptfilenames":[],"scripts":[]}),
            json!({"scriptfilenames":["A.uc","a.uc"],"scripts":["source","source"]}),
        ] {
            let mut value = manifest();
            value["files"][0]["changes"][0]["addtoclassorreplace"] = update;
            assert!(inspect_bytes(&container(&value.to_string(), &[])).is_err());
        }
    }

    #[test]
    fn retains_localization_requests_only_for_supported_localized_targets() -> Result<()> {
        let mut value = manifest();
        value["files"][0]["applytoalllocalizations"] = json!(true);
        assert!(inspect_bytes(&container(&value.to_string(), &[])).is_err());
        value["files"][0]["filename"] = json!("Startup_INT.pcc");
        assert!(inspect_bytes(&container(&value.to_string(), &[]))?.files[0].all_localizations);
        Ok(())
    }

    // @variants: both
    #[test]
    fn retains_all_le1_voice_and_text_variants_in_canonical_order() -> Result<()> {
        let expected = [
            "DE", "ES", "FE", "FR", "GE", "IE", "INT", "IT", "JA", "PL", "PLPC", "RA", "RU",
        ]
        .map(|code| format!("Startup_{code}.pcc"));
        let mut value = manifest();
        value["files"][0]["filename"] = json!("startup_plpc.PCC");
        value["files"][0]["applytoalllocalizations"] = json!(true);
        let plan = inspect_bytes(&container(&value.to_string(), &[]))?;
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].target_candidates, expected);
        assert_eq!(plan.files[0].target, "startup_plpc.PCC");
        let serialized = serde_json::to_string(&plan)?;
        assert_eq!(serde_json::from_str::<M3mPlan>(&serialized)?, plan);
        for filename in expected {
            value["files"][0]["filename"] = json!(filename.to_ascii_lowercase());
            value["files"][0]["applytoalllocalizations"] = json!(false);
            let explicit = inspect_bytes(&container(&value.to_string(), &[]))?;
            assert_eq!(explicit.files[0].target_candidates, [filename]);
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_unknown_localizations_and_other_engine_anchors() {
        for target in [
            "Startup_FRA.pcc",
            "Startup_POL.pcc",
            "Startup_XX.pcc",
            "Startup.pcc",
            "Startup_INT.pcc.extra",
            "../Startup_INT.pcc",
            "../system/Startup_INT.pcc",
            "~docs~/Startup_INT.pcc",
            "Mods/Startup_INT.pcc",
            "CookedPCConsole/Startup_INT.pcc",
        ] {
            let mut value = manifest();
            value["files"][0]["filename"] = json!(target);
            for localized in [false, true] {
                value["files"][0]["applytoalllocalizations"] = json!(localized);
                assert!(
                    inspect_bytes(&container(&value.to_string(), &[])).is_err(),
                    "{target}"
                );
            }
        }
    }

    #[test]
    fn verifies_source_identity_and_refuses_other_games() -> Result<()> {
        let root = tempdir()?;
        let data = container(&manifest().to_string(), &[]);
        let source = SourceFile {
            relative: "Example.m3m".into(),
            size: data.len() as u64,
            sha256: digest(&data),
        };
        fs::write(root.path().join(&source.relative), &data)?;
        inspect(root.path(), &source, Target::Le1)?;
        for target in [Target::Le2, Target::Le3] {
            assert!(inspect(root.path(), &source, target).is_err());
        }
        let mut changed = data;
        changed[0] ^= 1;
        fs::write(root.path().join(&source.relative), changed)?;
        assert!(inspect(root.path(), &source, Target::Le1).is_err());
        Ok(())
    }
}
