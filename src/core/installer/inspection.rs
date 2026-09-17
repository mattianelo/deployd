use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::core::archive;
use crate::dlog;
use crate::utils::fomod_resolver;

use super::{dazip, file_list};

pub(crate) enum PrepareResult {
    MassEffect {
        plan: Box<crate::core::game::mass_effect::package::PackagePlan>,
        bundled_launcher: bool,
        tmp_dir: TempDir,
    },
    Normal {
        file_list: Vec<(PathBuf, PathBuf)>,
        /// Original wrapper dir name stripped by detect_wrapper (e.g. `"modSkipMovies"`).
        /// Used by REDEngine path fixups to preserve the archive's folder name under `Mods/`.
        stripped_wrapper: Option<String>,
        tmp_dir: TempDir,
    },
    Fomod {
        config: fomod_resolver::FomodUiConfig,
        config_path: PathBuf,
        tmp_dir: TempDir,
    },
}

pub(crate) async fn prepare_mod(
    archive_path: &Path,
    manual_target: Option<crate::core::game::mass_effect::Target>,
    on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
    on_processing: Option<Box<dyn FnOnce() + Send>>,
) -> Result<PrepareResult> {
    dlog!("[deployd] prepare_mod: {}", archive_path.display());
    let path = archive_path.to_path_buf();
    let tmp_dir = tokio::task::spawn_blocking(move || {
        archive::extract_archive(&path, on_extract_progress)
            .with_context(|| format!("Extraction failed for: {}", path.display()))
    })
    .await
    .context("Extraction task panicked")??;

    if let Some(cb) = on_processing {
        cb();
    }

    let is_dazip = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dazip"));
    let stem = if is_dazip {
        archive_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string()
    } else {
        String::new()
    };

    tokio::task::spawn_blocking(move || {
        let extracted_root = tmp_dir.path();
        dlog!("[deployd] extracted to: {}", extracted_root.display());

        reject_headmorphs(extracted_root)?;
        if manual_target.is_some()
            || crate::core::game::mass_effect::package::discover_manifest(extracted_root)?.is_some()
        {
            let plan = crate::core::game::mass_effect::package::PackagePlan::inspect(
                extracted_root,
                manual_target,
            )?;
            plan.verify_sources(extracted_root)?;
            let bundled_launcher =
                crate::core::game::mass_effect::package::contains_launcher_manifest(
                    extracted_root,
                )?;
            return Ok(PrepareResult::MassEffect {
                plan: Box::new(plan),
                bundled_launcher,
                tmp_dir,
            });
        }

        if is_dazip {
            dazip::process_dazip_root(extracted_root, &stem)
                .context("Failed to process dazip archive")?;
        } else {
            dazip::expand_dazip_files_in_place(extracted_root)
                .context("Failed to expand nested .dazip files")?;
        }

        if let Some(config_path) = fomod_resolver::detect_fomod(extracted_root) {
            dlog!("[deployd] FOMOD detected: {}", config_path.display());
            let config = fomod_resolver::parse_fomod_config(&config_path).with_context(|| {
                format!("Failed to parse FOMOD config: {}", config_path.display())
            })?;
            dlog!(
                "[deployd] FOMOD config parsed: {} steps",
                config.steps.len()
            );
            Ok(PrepareResult::Fomod {
                config,
                config_path,
                tmp_dir,
            })
        } else {
            let (file_list, stripped_wrapper) = file_list::resolve_file_list(extracted_root)
                .context("Failed to resolve file list from extracted archive")?;
            dlog!("[deployd] normal mod: {} files resolved", file_list.len());
            Ok(PrepareResult::Normal {
                file_list,
                stripped_wrapper,
                tmp_dir,
            })
        }
    })
    .await
    .context("Post-extraction task panicked")?
}

fn reject_headmorphs(root: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.context("Cannot inspect archive contents")?;
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["headmorph", "me2headmorph", "me3headmorph"]
                    .iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            })
        {
            anyhow::bail!(
                "Headmorph support is deferred. This input cannot be installed as ordinary game files; no saves were modified"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[tokio::test]
    async fn mele_inspection_preserves_structure_before_generic_wrapper_detection() -> Result<()> {
        use crate::core::game::mass_effect::Target;
        use std::io::Write;
        let temp = tempfile::tempdir()?;
        let archive = temp.path().join("manual.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive)?);
        zip.start_file(
            "BioGame/CookedPCConsole/Engine.pcc",
            zip::write::SimpleFileOptions::default(),
        )?;
        zip.write_all(b"package")?;
        zip.finish()?;
        let prepared = prepare_mod(&archive, Some(Target::Le1), None, None).await?;
        let PrepareResult::MassEffect { plan, tmp_dir, .. } = prepared else {
            anyhow::bail!("MELE bypassed structured inspection")
        };
        assert_eq!(plan.files[0].destination, "CookedPCConsole/Engine.pcc");
        assert!(
            tmp_dir
                .path()
                .join("BioGame/CookedPCConsole/Engine.pcc")
                .is_file()
        );
        plan.verify_sources(tmp_dir.path())?;
        assert!(matches!(
            prepare_mod(&archive, None, None, None).await?,
            PrepareResult::Normal { .. }
        ));
        Ok(())
    }

    // @variants: both
    #[test]
    fn refuses_headmorphs_before_generic_archive_processing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for extension in ["headmorph", "ME2HeadMorph", "me3headmorph"] {
            let input = temp.path().join(format!("appearance.{extension}"));
            std::fs::write(&input, b"fixture")?;
            assert!(reject_headmorphs(temp.path()).is_err());
            assert_eq!(std::fs::read(&input)?, b"fixture");
            std::fs::remove_file(input)?;
        }
        std::fs::write(temp.path().join("normal.pcc"), b"fixture")?;
        reject_headmorphs(temp.path())?;
        Ok(())
    }
}
