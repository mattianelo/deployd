use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use crate::core::archive;
use crate::dlog;

/// Find every `.dazip` file under `dir` and expand it in-place.
///
/// Two DAZIP layouts are supported:
///
/// **Contents format** (standard DAO DAZIP):
/// ```text
/// Contents/addins/<uid>/…   → AddIns/<uid>/…
/// Contents/packages/…       → packages/…
/// Manifest.xml              → AddIns/<uid>/manifest.xml
/// ```
///
/// **Legacy format**:
/// ```text
/// package/…   → AddIns/<uid>/…
/// manifest.xml
/// ```
///
/// The UID comes from the first `UID="…"` attribute in the manifest.
pub(super) fn expand_dazip_files_in_place(dir: &Path) -> Result<Vec<super::DazipSource>> {
    let mut archives = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("dazip"))
        {
            archives.push(entry.into_path());
        }
    }
    archives.sort();
    let mut sources = Vec::new();
    for path in &archives {
        let parent = path.parent().context("DAZIP has no staging parent")?;
        let staging = tempfile::tempdir_in(parent)?;
        archive::extract_zip_to(path, staging.path())?;
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("addin");
        let uid = process_dazip_root(staging.path(), name)?;
        fs::remove_file(path)?;
        fs::rename(staging.path(), path).context("Cannot retain expanded DAZIP component")?;
        sources.push(super::DazipSource {
            root: path.clone(),
            key: format!("dazip:{uid}"),
        });
    }
    Ok(sources)
}

/// Process an already-extracted dazip root directory in-place.
///
/// Use this when the archive itself was a `.dazip` file (i.e. the user selected
/// a `.dazip` directly rather than a zip containing one). The extracted root
/// already contains the dazip's internal structure; this function reorganises it
/// into the same layout that `expand_dazip_files_in_place` produces.
pub(super) fn process_dazip_root(dir: &Path, dazip_name: &str) -> Result<String> {
    let manifest_path = find_file_case_insensitive(dir, "manifest.xml");

    let uid = manifest_path
        .as_ref()
        .and_then(|p| fs::read(p).ok())
        .and_then(|b| parse_dazip_uid(&b))
        .unwrap_or_else(|| dazip_name.to_string());
    anyhow::ensure!(
        !uid.is_empty() && !matches!(uid.as_str(), "." | "..") && !uid.contains(['/', '\\', ':']),
        "DAZIP manifest contains an invalid add-in UID"
    );
    dlog!("[deployd] dazip root UID: {uid}");

    let addins_uid_dir = dir.join("AddIns").join(&uid);

    let contents_dir =
        find_file_case_insensitive(dir, "contents").unwrap_or_else(|| dir.join("Contents"));
    if contents_dir.is_dir() {
        let addins_src = find_file_case_insensitive(&contents_dir, "addins")
            .unwrap_or_else(|| contents_dir.join("addins"));
        if addins_src.is_dir() {
            let addins_dst = dir.join("AddIns");
            fs::create_dir_all(&addins_dst)?;
            for entry in fs::read_dir(&addins_src)? {
                let entry = entry?;
                let dst = addins_dst.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    copy_dir_recursive(&entry.path(), &dst)?;
                } else {
                    fs::copy(entry.path(), &dst)?;
                }
            }
        }
        let packages_src = find_file_case_insensitive(&contents_dir, "packages")
            .unwrap_or_else(|| contents_dir.join("packages"));
        if packages_src.is_dir() {
            let packages_dst = dir.join("packages");
            copy_dir_recursive(&packages_src, &packages_dst)?;
        }
        fs::remove_dir_all(&contents_dir).with_context(|| {
            format!(
                "Failed to remove Contents/ after expansion: {}",
                contents_dir.display()
            )
        })?;
    } else {
        let package_dir = dir.join("package");
        if package_dir.is_dir() {
            if let Some(p) = addins_uid_dir.parent() {
                fs::create_dir_all(p)?;
            }
            fs::rename(&package_dir, &addins_uid_dir)
                .with_context(|| format!("Cannot move package/ to AddIns/{uid}"))?;
        } else {
            fs::create_dir_all(&addins_uid_dir)?;
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let name = entry.file_name();
                let name_lower = name.to_string_lossy().to_lowercase();
                if name_lower == "manifest.xml" || name_lower == "addins" {
                    continue;
                }
                let dest = addins_uid_dir.join(&name);
                fs::rename(entry.path(), &dest).context("Cannot move legacy DAZIP content")?;
            }
        }
    }

    if let Some(ref mp) = manifest_path {
        fs::create_dir_all(&addins_uid_dir)?;
        let dest_manifest = addins_uid_dir.join("manifest.xml");
        if mp != &dest_manifest {
            fs::copy(mp, &dest_manifest)
                .with_context(|| format!("Cannot copy manifest to {}", dest_manifest.display()))?;
            fs::remove_file(mp)?;
        }
    }

    Ok(uid)
}

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// Find a file by name (case-insensitive) directly inside `dir`.
fn find_file_case_insensitive(dir: &Path, name: &str) -> Option<PathBuf> {
    let lower = name.to_lowercase();
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().to_lowercase() == lower)
        .map(|e| e.path())
}

/// Parse the first `UID="…"` attribute from any `<AddIn*>` element in a
/// manifest.xml byte slice. Handles both `<AddIn>` (legacy) and `<AddInItem>`
/// (standard DAO DAZIP format).
fn parse_dazip_uid(data: &[u8]) -> Option<String> {
    use quick_xml::Reader;
    use quick_xml::XmlVersion;
    use quick_xml::events::Event;

    let src = std::str::from_utf8(data).ok()?;
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if e.name().as_ref().starts_with(b"AddIn") =>
            {
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == b"UID" {
                        return attr
                            .normalized_value(XmlVersion::Implicit1_0)
                            .ok()
                            .map(|s| s.into_owned());
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: appimage, snap
    #[test]
    fn rejects_manifest_uids_that_escape_the_addins_directory() -> Result<()> {
        for uid in [
            "../escape",
            "..",
            "/absolute",
            "nested/name",
            "nested\\name",
            "C:escape",
        ] {
            let temp = tempfile::tempdir()?;
            fs::write(
                temp.path().join("Manifest.xml"),
                format!("<AddInItem UID=\"{uid}\"/>"),
            )?;
            assert!(process_dazip_root(temp.path(), "fallback").is_err());
            assert!(!temp.path().join("AddIns").exists());
        }
        Ok(())
    }

    #[test]
    fn decodes_escaped_dazip_uid() {
        let manifest = br#"<AddInItem UID="mod&amp;id"/>"#;

        assert_eq!(parse_dazip_uid(manifest).as_deref(), Some("mod&id"));
    }
}
