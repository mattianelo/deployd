use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::Target;
use super::m3da::mount;
use super::m3za::read_input;
use super::package::{FileMapping, SourceFile};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Conditional {
    pub(crate) id: i32,
    pub(crate) script: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlotPlan {
    pub(crate) source: SourceFile,
    pub(crate) dlc: String,
    pub(crate) mount: i32,
    pub(crate) functions: Vec<Conditional>,
}

fn parse(text: &str) -> Result<Vec<Conditional>> {
    ensure!(
        text.len() <= 1024 * 1024 && !text.contains('\0'),
        "Invalid or oversized plot update"
    );
    let mut functions: Vec<Conditional> = Vec::new();
    let normalized = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    for line in normalized.lines() {
        if let Some(signature) = line.strip_prefix("public function bool F") {
            let (number, _) = signature
                .split_once('(')
                .context("Invalid plot conditional signature")?;
            let id: i32 = number.parse().context("Invalid plot conditional ID")?;
            ensure!(
                id > 0 && id.to_string() == number,
                "Plot IDs must be positive integers without leading zeros"
            );
            functions.push(Conditional {
                id,
                script: String::new(),
            });
            ensure!(functions.len() <= 4096, "Too many plot conditionals");
        }
        if let Some(function) = functions.last_mut() {
            function.script.push_str(line);
            function.script.push('\n');
        } else {
            ensure!(
                line.trim().is_empty() || line.trim_start().starts_with("//"),
                "Unexpected text before plot conditional"
            );
        }
    }
    ensure!(
        !functions.is_empty(),
        "Plot update contains no conditionals"
    );
    Ok(functions)
}

pub(super) fn inspect(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
    target: Target,
    format: &str,
) -> Result<Vec<PlotPlan>> {
    let mut plans = Vec::new();
    let mut mounts = BTreeMap::new();
    let source = |name: &str| {
        sources
            .iter()
            .find(|source| source.relative == name)
            .context("Plot input is absent from inspected sources")
    };
    for file in files
        .iter()
        .filter(|file| file.destination.to_ascii_lowercase().ends_with(".pmu"))
    {
        ensure!(
            matches!(target, Target::Le1 | Target::Le2),
            "Plot-manager updates support LE1 and LE2 only"
        );
        let parts: Vec<_> = file.destination.split('/').collect();
        ensure!(
            parts.len() == 4
                && parts[0] == "DLC"
                && parts[1].to_ascii_lowercase().starts_with("dlc_mod_")
                && parts[2].eq_ignore_ascii_case("CookedPCConsole")
                && (parts[3] == "PlotManagerUpdate.pmu"
                    || (format == "9.2" && parts[3].ends_with(".pmu"))),
            "Plot updates require a custom DLC CookedPCConsole directory; additional PMU filenames require moddesc 9.2"
        );
        let dlc = parts[1];
        let mount_path = if target == Target::Le1 {
            format!("DLC/{dlc}/AutoLoad.ini")
        } else {
            format!("DLC/{dlc}/CookedPCConsole/Mount.dlc")
        };
        let autoload = files
            .iter()
            .find(|file| file.destination.eq_ignore_ascii_case(&mount_path))
            .context("Plot update DLC is missing its mount file")?;
        let mount = if target == Target::Le1 {
            mount(std::str::from_utf8(&read_input(
                root,
                source(&autoload.source)?,
                1024 * 1024,
            )?)?)?
        } else {
            super::m3to::mount(root, source(&autoload.source)?, target)?
        };
        ensure!(
            mounts
                .insert(mount, dlc)
                .is_none_or(|previous| previous == dlc),
            "Ambiguous plot DLC mount priority"
        );
        let source = source(&file.source)?;
        plans.push(PlotPlan {
            source: source.clone(),
            dlc: dlc.to_owned(),
            mount,
            functions: parse(std::str::from_utf8(&read_input(
                root,
                source,
                1024 * 1024,
            )?)?)?,
        });
    }
    plans.sort_by_key(|plan| plan.mount);
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use tempfile::{TempDir, tempdir};

    use super::*;

    fn fixture(mounts: &[i32]) -> Result<(TempDir, Vec<SourceFile>, Vec<FileMapping>)> {
        let root = tempdir()?;
        let mut sources = Vec::new();
        let mut files = Vec::new();
        for (index, mount) in mounts.iter().enumerate() {
            for (name, text) in [("AutoLoad.ini", format!("[ME1DLCMOUNT]\nModMount={mount}\n")),
                ("CookedPCConsole/PlotManagerUpdate.pmu", "public function bool F9(BioWorldInfo bioWorld, int Argument)\n{ return true; }".into())] {
                let relative = format!("DLC/DLC_MOD_TEST_{index}/{name}");
                let path = root.path().join(&relative);
                std::fs::create_dir_all(path.parent().context("Missing fixture parent")?)?;
                std::fs::write(path, &text)?;
                sources.push(SourceFile { relative: relative.clone(), size: text.len() as u64, sha256: format!("{:x}", Sha256::digest(text.as_bytes())) });
                files.push(FileMapping { source: relative.clone(), destination: relative });
            }
        }
        Ok((root, sources, files))
    }

    // @variants: both
    #[test]
    fn accepts_additional_plot_manifests_only_at_feature_level_92() -> Result<()> {
        let (root, mut sources, mut files) = fixture(&[5])?;
        let mut extra = sources[1].clone();
        extra.relative = extra.relative.replace("PlotManagerUpdate", "Additional");
        std::fs::copy(
            root.path().join(&sources[1].relative),
            root.path().join(&extra.relative),
        )?;
        files.push(FileMapping {
            source: extra.relative.clone(),
            destination: extra.relative.clone(),
        });
        sources.push(extra);
        assert!(inspect(root.path(), &sources, &files, Target::Le1, "9.1").is_err());
        let plans = inspect(root.path(), &sources, &files, Target::Le1, "9.2")?;
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].dlc, plans[1].dlc);
        assert_eq!(plans[0].mount, plans[1].mount);
        assert!(inspect(root.path(), &sources, &files, Target::Le3, "9.2").is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn orders_plot_contributions_by_mount_and_rejects_ambiguous_mounts() -> Result<()> {
        let (root, sources, files) = fixture(&[10, 5])?;
        let plans = inspect(root.path(), &sources, &files, Target::Le1, "9.1")?;
        assert_eq!(
            plans.iter().map(|plan| plan.mount).collect::<Vec<_>>(),
            [5, 10]
        );
        let (root, sources, files) = fixture(&[5, 5])?;
        assert!(inspect(root.path(), &sources, &files, Target::Le1, "9.1").is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_foreign_plot_targets_missing_mounts_and_changed_sources() -> Result<()> {
        let (root, sources, files) = fixture(&[5])?;
        for game in [Target::Le2, Target::Le3] {
            assert!(inspect(root.path(), &sources, &files, game, "9.1").is_err());
        }
        assert!(inspect(root.path(), &sources, &files[1..], Target::Le1, "9.1").is_err());
        for destination in [
            "../system/PlotManagerUpdate.pmu",
            "~docs~/PlotManagerUpdate.pmu",
            "Mods/PlotManagerUpdate.pmu",
            "DLC/DLC_MOD_TEST_0/CookedPCConsole/Extra.pmu",
        ] {
            let mut changed = files.clone();
            changed[1].destination = destination.into();
            assert!(inspect(root.path(), &sources, &changed, Target::Le1, "9.1").is_err());
        }
        std::fs::write(root.path().join(&sources[1].relative), "changed")?;
        assert!(inspect(root.path(), &sources, &files, Target::Le1, "9.1").is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn preserves_plot_order_and_repeated_definitions() -> Result<()> {
        let functions = parse(
            "// header\r\npublic function bool F9(BioWorldInfo bioWorld, int Argument)\r\n{ return true; }\r\npublic function bool F9(BioWorldInfo bioWorld, int Argument)\r\n{ return false; }\r\n",
        )?;
        assert_eq!(
            functions
                .iter()
                .map(|function| function.id)
                .collect::<Vec<_>>(),
            [9, 9]
        );
        assert!(functions[1].script.ends_with("{ return false; }\n"));
        assert_eq!(
            parse(
                &functions
                    .iter()
                    .map(|function| function.script.replace('\n', "\r"))
                    .collect::<String>()
            )?,
            functions
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_invalid_plot_ids_and_unrecognized_text() {
        for id in ["0", "01", "-1", "2147483648", "+1", "one", "1 "] {
            assert!(parse(&format!("public function bool F{id}() {{ return true; }}")).is_err());
        }
        for text in [
            "",
            "function F1() {}",
            "ignored\npublic function bool F1() {}",
            "\0",
        ] {
            assert!(parse(text).is_err());
        }
    }
}
