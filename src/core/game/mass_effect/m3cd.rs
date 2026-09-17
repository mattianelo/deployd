use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Target;
use super::helper::FileIdentity;
use super::journal::Control;
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
    target: Target,
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
        let mount = if target == Target::Le1 {
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
            mount
        } else {
            ensure!(
                target == Target::Le2,
                "M3CD installation currently supports LE1 and LE2"
            );
            0
        };
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

pub(super) fn apply_dlc_config(
    root: &Path,
    plans: &[M3cdPlan],
    managed: &mut BTreeMap<String, SourceFile>,
    current: &mut BTreeMap<String, FileIdentity>,
    control: &Control,
) -> Result<()> {
    for plan in plans {
        for edit in &plan.edits {
            control.check()?;
            let expected = format!("BioGame/DLC/{}/CookedPCConsole/{}", plan.dlc, edit.file);
            let relative = managed
                .keys()
                .find(|path| path.eq_ignore_ascii_case(&expected))
                .cloned()
                .with_context(|| {
                    format!(
                        "M3CD target '{}' is missing from custom DLC '{}'",
                        edit.file, plan.dlc
                    )
                })?;
            let path = root.join(&relative);
            let text = fs::read_to_string(&path)
                .with_context(|| format!("Cannot read M3CD target '{relative}'"))?;
            let updated = apply_edit(&text, edit)?;
            fs::write(&path, updated.as_bytes())
                .with_context(|| format!("Cannot update M3CD target '{relative}'"))?;
            fs::File::open(&path)?.sync_all()?;
            let identity = SourceFile {
                relative: relative.clone(),
                size: updated.len() as u64,
                sha256: format!("{:x}", Sha256::digest(updated.as_bytes())),
            };
            let game_path = relative
                .strip_prefix("BioGame/")
                .context("M3CD target is outside BioGame")?
                .to_owned();
            managed.insert(relative.clone(), identity.clone());
            current.insert(
                game_path.clone(),
                FileIdentity {
                    path: game_path,
                    size: identity.size,
                    sha256: identity.sha256,
                },
            );
        }
    }
    Ok(())
}

fn apply_edit(text: &str, edit: &ConfigEdit) -> Result<String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing_newline = text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let header = format!("[{}]", edit.section);
    let sections: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim().eq_ignore_ascii_case(&header))
        .map(|(index, _)| index)
        .collect();
    ensure!(sections.len() <= 1, "M3CD target section is ambiguous");
    let start = match sections.first() {
        Some(index) => index + 1,
        None => {
            if !lines.is_empty() && !lines.last().is_some_and(String::is_empty) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.len()
        }
    };
    let end = lines[start..]
        .iter()
        .position(|line| {
            let line = line.trim();
            line.starts_with('[') && line.ends_with(']')
        })
        .map_or(lines.len(), |offset| start + offset);
    let matches = |line: &str| {
        let line = line.trim();
        if line.starts_with(';') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        let key = key
            .chars()
            .next()
            .filter(|prefix| "+-.!>".contains(*prefix))
            .map_or(key, |prefix| &key[prefix.len_utf8()..]);
        if key.eq_ignore_ascii_case(&edit.key) {
            Some(value.trim().to_owned())
        } else {
            None
        }
    };
    let insertion = |lines: &[String], mut end: usize| {
        while end > start && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
        end
    };
    match edit.action {
        '-' => {
            let mut index = start;
            let mut current_end = end;
            while index < current_end {
                if matches(&lines[index]).is_some_and(|value| value == edit.value) {
                    lines.remove(index);
                    current_end -= 1;
                } else {
                    index += 1;
                }
            }
        }
        '!' | '>' => {
            let mut index = start;
            let mut current_end = end;
            while index < current_end {
                if matches(&lines[index]).is_some() {
                    lines.remove(index);
                    current_end -= 1;
                } else {
                    index += 1;
                }
            }
            if edit.action == '>' {
                lines.insert(
                    insertion(&lines, current_end),
                    format!("{}={}", edit.key, edit.value),
                );
            }
        }
        '+' => {
            if !lines[start..end]
                .iter()
                .any(|line| matches(line).is_some_and(|value| value == edit.value))
            {
                lines.insert(
                    insertion(&lines, end),
                    format!("+{}={}", edit.key, edit.value),
                );
            }
        }
        '.' => lines.insert(
            insertion(&lines, end),
            format!(".{}={}", edit.key, edit.value),
        ),
        _ => anyhow::bail!("Unknown M3CD operation"),
    }
    let mut result = lines.join(newline);
    if trailing_newline {
        result.push_str(newline);
    }
    Ok(result)
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

    // @variants: both
    #[test]
    fn applies_dlc_config_edits_without_requiring_optional_targets() -> Result<()> {
        let original = "[SFXGame.BioGlobalVariableTable]\r\n+TimedPlotUnlocks=Old\r\n\r\n[Other]\r\nKey=Value\r\n";
        let edit = ConfigEdit {
            file: "BioGame.ini".into(),
            section: "SFXGame.BioGlobalVariableTable".into(),
            key: "TimedPlotUnlocks".into(),
            action: '>',
            value: "New".into(),
        };
        assert_eq!(
            apply_edit(original, &edit)?,
            "[SFXGame.BioGlobalVariableTable]\r\nTimedPlotUnlocks=New\r\n\r\n[Other]\r\nKey=Value\r\n"
        );
        Ok(())
    }
}
