use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::m3da::{mount, read_text};
use super::package::{FileMapping, SourceFile};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigEdit {
    pub(crate) file: String,
    pub(crate) section: String,
    pub(crate) key: String,
    pub(crate) action: char,
    pub(crate) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct M3cdPlan {
    pub(crate) manifest: String,
    pub(crate) dlc: String,
    pub(crate) mount: i32,
    pub(crate) edits: Vec<ConfigEdit>,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.'))
}

fn parse(text: &str) -> Result<Vec<ConfigEdit>> {
    ensure!(text.len() <= 1024 * 1024, "M3CD exceeds 1 MiB");
    let mut header = None;
    let mut edits = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        ensure!(
            !line.chars().any(|ch| ch.is_control() && ch != '\t'),
            "Control character in M3CD"
        );
        if line.starts_with('[') {
            let section = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
                .context("Unterminated M3CD section")?;
            let (file, section) = section
                .split_once(' ')
                .context("M3CD sections require an INI filename and section name")?;
            ensure!(
                identifier(file)
                    && file.to_ascii_lowercase().ends_with(".ini")
                    && !file.starts_with('.')
                    && identifier(section)
                    && !section.starts_with('.'),
                "Unsupported M3CD asset or section name"
            );
            header = Some((file, section));
            continue;
        }
        let (file, section) = header.context("M3CD entry must follow a section")?;
        let (key, value) = line
            .split_once('=')
            .context("M3CD entry must contain a key/value pair")?;
        let key = key.trim();
        let (action, key) = match key.as_bytes().first() {
            Some(&prefix) if b"+-.!>".contains(&prefix) => (char::from(prefix), &key[1..]),
            _ => ('.', key),
        };
        ensure!(
            identifier(key) && !key.starts_with('.') && !key.contains(".."),
            "Unsupported M3CD key or double-typed operation"
        );
        edits.push(ConfigEdit {
            file: file.to_owned(),
            section: section.to_owned(),
            key: key.to_owned(),
            action,
            value: value.trim().to_owned(),
        });
        ensure!(edits.len() <= 16384, "M3CD has too many operations");
    }
    ensure!(!edits.is_empty(), "M3CD contains no operations");
    Ok(edits)
}

pub(super) fn inspect(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
) -> Result<Vec<M3cdPlan>> {
    let index: BTreeMap<_, _> = sources
        .iter()
        .map(|source| (source.relative.as_str(), source))
        .collect();
    let source = |file: &FileMapping| {
        index
            .get(file.source.as_str())
            .copied()
            .context("M3CD source is absent from the inspected package")
    };
    let mut plans = Vec::new();
    let mut mounts = BTreeMap::new();
    for file in files
        .iter()
        .filter(|file| file.destination.to_ascii_lowercase().ends_with(".m3cd"))
    {
        let parts: Vec<_> = file.destination.split('/').collect();
        ensure!(
            parts.len() == 4
                && parts[0].eq_ignore_ascii_case("DLC")
                && parts[1].to_ascii_lowercase().starts_with("dlc_mod_")
                && parts[2].eq_ignore_ascii_case("CookedPCConsole")
                && parts[3].to_ascii_lowercase().starts_with("configdelta-")
                && parts[3].len() > 17,
            "M3CD requires ConfigDelta-<name>.m3cd directly in a custom DLC's CookedPCConsole directory"
        );
        let dlc = parts[1];
        let autoload = files
            .iter()
            .find(|file| {
                file.destination
                    .eq_ignore_ascii_case(&format!("DLC/{dlc}/AutoLoad.ini"))
            })
            .context("M3CD DLC is missing AutoLoad.ini")?;
        let mount = mount(&read_text(root, source(autoload)?)?)?;
        ensure!(
            mounts
                .insert(mount, dlc.to_ascii_lowercase())
                .is_none_or(|owner| owner.eq_ignore_ascii_case(dlc)),
            "Multiple DLCs have the same M3CD mount priority"
        );
        plans.push(M3cdPlan {
            manifest: file.source.clone(),
            dlc: dlc.to_owned(),
            mount,
            edits: parse(&read_text(root, source(file)?)?)?,
        });
    }
    plans.sort_by_key(|plan| (plan.mount, plan.manifest.to_ascii_lowercase()));
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn preserves_operation_order_and_struct_values() -> Result<()> {
        let edits = parse(
            "; comment\n[BioUI.ini Engine.BioSFManager]\n-Key=(Tag=Options,\tName=Old)\n+Key=(Tag=Options, Name=New)\nKey=duplicate\n.Key=duplicate\n!Other=ignored\n>Single=\n",
        )?;
        assert_eq!(
            edits.iter().map(|edit| edit.action).collect::<String>(),
            "-+..!>"
        );
        assert_eq!(edits[0].value, "(Tag=Options,\tName=Old)");
        assert_eq!(edits[1].value, "(Tag=Options, Name=New)");
        assert!(edits[5].value.is_empty());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_unknown_and_malformed_config_semantics() {
        for text in [
            "",
            "[BioUI.ini Engine.UI]",
            "Key=Value",
            "[BioUI.ini]\nKey=Value",
            "[../BioUI.ini Engine.UI]\nKey=Value",
            "[BioUI.ini Engine.UI\nKey=Value",
            "[BioUI.ini Engine.UI]\nMissing equals",
            "[BioUI.ini Engine.UI]\n++Key=Value",
            "[BioUI.ini Engine.UI]\n?Key=Value",
            "[BioUI.ini Engine.UI]\n=Value",
            "[BioUI.ini Engine.UI]\nKey=Val\0ue",
            "[BioUI.ini Engine.UI]\n+=Value",
        ] {
            assert!(parse(text).is_err(), "accepted: {text:?}");
        }
    }
}
