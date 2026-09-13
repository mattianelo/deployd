use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use ini::{Ini, ParseOption};
use serde::{Deserialize, Serialize};

use super::manifest::relative_path;
use super::package::{FileMapping, SourceFile};

pub(super) const TARGETS: &[&str] = &[
    "Engine.pcc",
    "SFXGame.pcc",
    "EntryMenu.pcc",
    "BIOG_2DA_UNC_AreaMap_X.pcc",
    "BIOG_2DA_UNC_GalaxyMap_X.pcc",
    "BIOG_2DA_UNC_GamerProfile_X.pcc",
    "BIOG_2DA_UNC_Movement_X.pcc",
    "BIOG_2DA_UNC_Music_X.pcc",
    "BIOG_2DA_UNC_Talents_X.pcc",
    "BIOG_2DA_UNC_TreasureTables_X.pcc",
    "BIOG_2DA_UNC_UI_X.pcc",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TableMerge {
    pub(crate) target: String,
    pub(crate) source: String,
    pub(crate) tables: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct M3daPlan {
    pub(crate) manifest: String,
    pub(crate) dlc: String,
    pub(crate) mount: i32,
    pub(crate) merges: Vec<TableMerge>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    packagefile: String,
    mergepackagefile: String,
    mergetables: Vec<String>,
    #[serde(default, rename = "comment")]
    _comment: Option<String>,
}

fn parse(text: &str) -> Result<Vec<Entry>> {
    ensure!(text.len() <= 1024 * 1024, "M3DA exceeds the 1 MiB limit");
    let entries: Vec<Entry> = serde_json::from_slice(&without_trailing_commas(text.as_bytes()))
        .context("Invalid or unsupported M3DA manifest")?;
    ensure!(
        !entries.is_empty() && entries.len() <= 1024,
        "Invalid M3DA entry count"
    );
    for entry in &entries {
        ensure!(
            TARGETS
                .iter()
                .any(|target| target.eq_ignore_ascii_case(&entry.packagefile)),
            "Unsupported M3DA target '{}'",
            entry.packagefile
        );
        ensure!(
            relative_path(&entry.mergepackagefile)? == entry.mergepackagefile
                && !entry.mergepackagefile.contains('/')
                && entry
                    .mergepackagefile
                    .to_ascii_lowercase()
                    .ends_with(".pcc"),
            "M3DA mergepackagefile must be a package filename"
        );
        ensure!(
            !entry.mergetables.is_empty() && entry.mergetables.len() <= 4096,
            "Invalid M3DA table count"
        );
        let mut names = BTreeSet::new();
        for table in &entry.mergetables {
            ensure!(
                !table.is_empty()
                    && table.len() <= 1024
                    && !table.chars().any(char::is_control)
                    && table.split('.').all(|part| !part.is_empty())
                    && names.insert(table.to_ascii_lowercase()),
                "Invalid or duplicate M3DA table reference"
            );
            let object = table.rsplit('.').next().unwrap_or_default();
            let name = object
                .rsplit_once('_')
                .filter(|(_, number)| {
                    (number.len() == 1 || !number.starts_with('0'))
                        && number.bytes().all(|byte| byte.is_ascii_digit())
                        && number.parse::<i32>().is_ok_and(|value| value < i32::MAX)
                })
                .map_or(object, |(name, _)| name);
            ensure!(
                name.len() > 5 && name.ends_with("_part"),
                "M3DA table '{table}' must end in _part with an optional instance number"
            );
        }
    }
    Ok(entries)
}

fn without_trailing_commas(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut quoted = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else if byte == b'"' {
            quoted = true;
        } else if byte == b','
            && bytes[index + 1..]
                .iter()
                .find(|byte| !byte.is_ascii_whitespace())
                .is_some_and(|byte| matches!(byte, b']' | b'}'))
        {
            continue;
        }
        output.push(byte);
    }
    output
}

pub(super) fn read_text(root: &Path, source: &SourceFile) -> Result<String> {
    ensure!(source.size <= 1024 * 1024, "LE1 merge input exceeds 1 MiB");
    let mut text = String::new();
    File::open(root.join(&source.relative))?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)?;
    ensure!(text.len() <= 1024 * 1024, "LE1 merge input changed size");
    Ok(text)
}

pub(super) fn mount(text: &str) -> Result<i32> {
    let ini = Ini::load_from_str_opt(
        text,
        ParseOption {
            enabled_quote: false,
            enabled_escape: false,
            ..ParseOption::default()
        },
    )?;
    let mut values = Vec::new();
    for (section, properties) in &ini {
        if section.is_some_and(|section| section.eq_ignore_ascii_case("ME1DLCMOUNT")) {
            for (key, value) in properties.iter() {
                if key.eq_ignore_ascii_case("ModMount") {
                    values.push(value);
                }
            }
        }
    }
    ensure!(
        values.len() == 1,
        "LE1 merges require exactly one [ME1DLCMOUNT] ModMount value in AutoLoad.ini"
    );
    values[0]
        .trim()
        .parse()
        .context("Invalid LE1 DLC mount value")
}

pub(super) fn inspect(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
) -> Result<Vec<M3daPlan>> {
    let index: BTreeMap<_, _> = sources
        .iter()
        .map(|source| (source.relative.as_str(), source))
        .collect();
    let source = |mapping: &FileMapping| {
        index
            .get(mapping.source.as_str())
            .copied()
            .context("M3DA source is absent from the inspected package")
    };
    let mut plans = Vec::new();
    let mut mounts = BTreeMap::new();
    for file in files
        .iter()
        .filter(|file| file.destination.to_ascii_lowercase().ends_with(".m3da"))
    {
        let parts: Vec<_> = file.destination.split('/').collect();
        ensure!(
            parts.len() >= 4
                && parts[0].eq_ignore_ascii_case("DLC")
                && parts[1].to_ascii_lowercase().starts_with("dlc_mod_")
                && parts[2].eq_ignore_ascii_case("CookedPCConsole"),
            "M3DA must be installed beneath a custom DLC's CookedPCConsole directory"
        );
        let dlc = parts[1];
        let filename = parts.last().context("Missing M3DA filename")?;
        ensure!(
            filename
                .to_ascii_lowercase()
                .starts_with(&format!("{}-", dlc.to_ascii_lowercase()))
                && filename.len() > dlc.len() + 6,
            "M3DA filename must begin with its DLC name followed by a dash and a suffix"
        );
        let prefix = format!("DLC/{dlc}/CookedPCConsole/").to_ascii_lowercase();
        let autoload = files
            .iter()
            .find(|file| {
                file.destination
                    .eq_ignore_ascii_case(&format!("DLC/{dlc}/AutoLoad.ini"))
            })
            .context("M3DA DLC is missing AutoLoad.ini")?;
        let mount = mount(&read_text(root, source(autoload)?)?)?;
        ensure!(
            mounts
                .insert(mount, dlc.to_ascii_lowercase())
                .is_none_or(|owner| owner.eq_ignore_ascii_case(dlc)),
            "Multiple DLCs have the same M3DA mount priority"
        );
        let mut merges = Vec::new();
        for entry in parse(&read_text(root, source(file)?)?)? {
            let matches: Vec<_> =
                files
                    .iter()
                    .filter(|file| {
                        file.destination.to_ascii_lowercase().starts_with(&prefix)
                            && file.destination.rsplit('/').next().is_some_and(|name| {
                                name.eq_ignore_ascii_case(&entry.mergepackagefile)
                            })
                    })
                    .collect();
            ensure!(
                matches.len() == 1,
                "M3DA source package '{}' is missing or ambiguous in its DLC",
                entry.mergepackagefile
            );
            merges.push(TableMerge {
                target: entry.packagefile,
                source: matches[0].source.clone(),
                tables: entry.mergetables,
            });
        }
        plans.push(M3daPlan {
            manifest: file.source.clone(),
            dlc: dlc.to_owned(),
            mount,
            merges,
        });
    }
    if !plans.is_empty() {
        ensure!(
            !files.iter().any(|file| {
                file.destination.to_ascii_lowercase().starts_with("dlc/")
                    && file.destination.rsplit('/').next().is_some_and(|name| {
                        TARGETS
                            .iter()
                            .any(|target| target.eq_ignore_ascii_case(name))
                    })
            }),
            "DLC package overrides conflict with M3DA's basegame targets"
        );
    }
    plans.sort_by(|left, right| {
        (left.mount, left.manifest.to_ascii_lowercase())
            .cmp(&(right.mount, right.manifest.to_ascii_lowercase()))
    });
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"[{"packagefile":"Engine.pcc","mergepackagefile":"Tables.pcc","mergetables":["Group.Values_part_1",],},]"#;

    #[test]
    fn accepts_trailing_commas_without_changing_quoted_values() {
        let text = VALID.replace(
            "\"packagefile\":",
            "\"comment\":\"Keep ,] and \\\"quoted\\\" text\",\"packagefile\":",
        );
        let entries = parse(&text).unwrap();
        assert_eq!(
            entries[0]._comment.as_deref(),
            Some("Keep ,] and \"quoted\" text")
        );
        assert_eq!(entries[0].mergetables, ["Group.Values_part_1"]);
    }

    #[test]
    fn rejects_unknown_fields_duplicate_keys_and_invalid_table_targets() {
        for text in [
            VALID.replace("Engine.pcc", "../Engine.pcc"),
            VALID.replace("Tables.pcc", "../Tables.pcc"),
            VALID.replace("Values_part_1", "Values_1"),
            VALID.replace("Values_part_1", "Values_part_01"),
            VALID.replace("Values_part_1", "Values_part_2147483647"),
            VALID.replace("\"packagefile\":", "\"future\":true,\"packagefile\":"),
            VALID.replace(
                "\"packagefile\":",
                "\"packagefile\":\"Engine.pcc\",\"packagefile\":",
            ),
            VALID.replace("_1\",", "_1\",,"),
        ] {
            assert!(parse(&text).is_err(), "{text}");
        }
    }

    #[test]
    fn reads_mount_case_insensitively_and_rejects_missing_or_duplicate_values() {
        assert_eq!(mount("[me1dlcmount]\nmodmount = 5").unwrap(), 5);
        for text in [
            "",
            "[ME1DLCMOUNT]\nModMount=no",
            "[ME1DLCMOUNT]\nModMount=1\nModMount=2",
        ] {
            assert!(mount(text).is_err());
        }
    }
}
