use std::{fs, io::Read, path::Path};

use anyhow::{Context, Result, ensure};

use super::morph::{self, Preset};

pub(crate) fn separate(root: &Path) -> Result<Vec<Preset>> {
    let mut presets = Vec::new();
    let mut paths = Vec::new();
    let mut total = 0;
    for entry in walkdir::WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry?;
        ensure!(
            entry.file_type().is_file() || entry.file_type().is_dir(),
            "Archive contains a link or special file"
        );
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "ron" | "headmorph" | "me2headmorph" | "me3headmorph"
        ) {
            continue;
        }
        let mut bytes = Vec::new();
        fs::File::open(entry.path())?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        let name = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .into_owned();
        match morph::parse(&bytes, name.clone()) {
            Ok(preset) => {
                total += bytes.len();
                ensure!(
                    presets.len() < 64 && total <= 64 * 1024 * 1024,
                    "Archive contains too many appearance presets"
                );
                presets.push(preset);
                paths.push(entry.into_path());
            }
            Err(error)
                if ext != "ron" || String::from_utf8_lossy(&bytes).contains("lod0_vertices") =>
            {
                return Err(error).with_context(|| format!("Invalid appearance preset {name}"));
            }
            Err(_) => {}
        }
    }
    for path in paths {
        fs::remove_file(path)?;
    }
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(presets)
}

pub(crate) fn has_payload(root: &Path) -> Result<bool> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "txt" | "md" | "pdf" | "png" | "jpg" | "jpeg" | "gif" | "webp"
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_presets_and_keeps_game_assets_and_ordinary_ron() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("hair.pcc"), b"game asset")?;
        fs::write(temp.path().join("settings.ron"), b"(enabled: true)")?;
        fs::write(
            temp.path().join("face.headmorph"),
            include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph"),
        )?;
        let presets = separate(temp.path())?;
        assert_eq!(presets.len(), 1);
        assert!(!temp.path().join("face.headmorph").exists());
        assert!(temp.path().join("settings.ron").exists());
        assert!(has_payload(temp.path())?);
        Ok(())
    }
    #[test]
    fn rejects_bad_presets_without_removing_valid_ones() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("valid.me2headmorph"),
            include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph"),
        )?;
        fs::write(temp.path().join("broken.headmorph"), b"bad")?;
        assert!(separate(temp.path()).is_err());
        assert!(temp.path().join("valid.me2headmorph").exists());
        Ok(())
    }
}
