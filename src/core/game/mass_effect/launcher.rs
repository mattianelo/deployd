use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::journal::{Identity, files};
use super::operation::Control;
use super::package::SourceFile;

mod package;
mod service;
pub(crate) use package::inspect_bundle;
pub(crate) use service::{Action, Snapshot, add_bundled, apply, load};

#[derive(Debug)]
pub(crate) struct Bundled {
    pub(crate) entry: Entry,
    pub(crate) source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(super) source_sha256: String,
    pub(super) approval: String,
    pub(super) files: Vec<Mapping>,
    pub(super) sources: Vec<SourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Mapping {
    pub(super) source: String,
    pub(super) destination: String,
    pub(super) identity: Identity,
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Inspected {
    pub(crate) entry: Entry,
    pub(crate) source: Option<tempfile::TempDir>,
}

#[cfg(test)]
impl Drop for Inspected {
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            // Closing an installer dialog must not delete an extracted archive on GTK.
            std::thread::spawn(move || drop(source));
        }
    }
}

pub(super) fn destination(path: &str) -> Result<()> {
    super::baseline::relative(path)?;
    ensure!(
        ![
            "bink2w64.dll",
            "bink2w64_original.dll",
            "MassEffectLauncher.exe"
        ]
        .iter()
        .any(|reserved| path.eq_ignore_ascii_case(reserved)),
        "This launcher file is reserved; executable replacement is not supported"
    );
    let extension = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ensure!(
        [
            "asi", "dll", "ini", "json", "toml", "xml", "txt", "cfg", "swf", "swd", "bik", "bk2",
            "png", "jpg", "dds"
        ]
        .contains(&extension.as_str()),
        "Unsupported launcher file; installers and scripts cannot be deployed"
    );
    ensure!(
        path.starts_with("Content/") || path.starts_with("ASI/") || !path.contains('/'),
        "Launcher mods must target Content, ASI, or files directly inside Game/Launcher"
    );
    if super::binary::executable(path) {
        ensure!(
            !path.starts_with("Content/"),
            "Launcher plugins cannot be installed as content"
        );
    }
    if extension == "asi" {
        ensure!(
            path.strip_prefix("ASI/")
                .is_some_and(|name| !name.contains('/')),
            "Launcher ASIs must be directly inside the ASI folder"
        );
    }
    Ok(())
}

pub(super) fn validate_entries(entries: &[Entry]) -> Result<()> {
    ensure!(entries.len() <= 256, "Too many shared launcher mods");
    let mut ids = BTreeSet::new();
    let mut destinations = BTreeMap::new();
    for entry in entries {
        ensure!(
            Uuid::parse_str(&entry.id)?.to_string() == entry.id && ids.insert(&entry.id),
            "Invalid or duplicate launcher mod identity"
        );
        ensure!(
            !entry.name.trim().is_empty()
                && entry.name.len() <= 1024
                && entry.files.len() <= 10000
                && entry.sources.len() <= 10000,
            "Invalid launcher mod metadata"
        );
        ensure!(
            entry.approval == entry.source_sha256
                && entry.source_sha256 == super::package::tree_digest(&entry.sources),
            "Launcher mod requires approval for this exact source"
        );
        let mut sources = BTreeSet::new();
        for file in &entry.sources {
            super::baseline::relative(&file.relative)?;
            ensure!(
                sources.insert(file.relative.to_lowercase()),
                "Case-colliding launcher source files"
            );
            Identity {
                size: file.size,
                sha256: file.sha256.clone(),
            }
            .validate()?;
        }
        let mut unique = BTreeSet::new();
        for file in &entry.files {
            destination(&file.destination)?;
            file.identity.validate()?;
            ensure!(
                unique.insert(file.destination.to_lowercase()),
                "Duplicate launcher destinations"
            );
            ensure!(
                entry
                    .sources
                    .iter()
                    .any(|source| source.relative == file.source
                        && source.size == file.identity.size
                        && source.sha256 == file.identity.sha256),
                "Launcher mapping differs from its source identity"
            );
            for prefix in Path::new(&file.destination)
                .ancestors()
                .filter(|path| !path.as_os_str().is_empty())
            {
                let prefix = prefix.to_str().context("Invalid launcher destination")?;
                let value = (prefix.to_owned(), prefix == file.destination);
                ensure!(
                    destinations
                        .insert(prefix.to_lowercase(), value.clone())
                        .is_none_or(|old| old == value),
                    "Launcher paths collide by case or file/directory type"
                );
            }
        }
    }
    Ok(())
}

pub(super) fn source_root(data: &Path, hash: &str) -> PathBuf {
    data.join("mele-launcher-sources").join(hash)
}

fn retain(data: &Path, source: &Path, entry: &Entry, control: &Control) -> Result<()> {
    validate_entries(std::slice::from_ref(entry))?;
    let cache = data.join("mele-launcher-sources");
    files::create_directory(&cache)?;
    let destination = source_root(data, &entry.source_sha256);
    if destination.try_exists()? {
        return verify_source(&destination, entry, control);
    }
    let temp = tempfile::Builder::new()
        .prefix("incoming-")
        .tempdir_in(&cache)?;
    for file in &entry.sources {
        control.check()?;
        files::copy(
            source,
            &file.relative,
            temp.path(),
            &file.relative,
            &Identity {
                size: file.size,
                sha256: file.sha256.clone(),
            },
            control,
        )?;
    }
    verify_source(temp.path(), entry, control)?;
    files::sync(temp.path())?;
    std::fs::rename(temp.path(), &destination)?;
    files::sync(&cache)
}

fn verify_source(root: &Path, entry: &Entry, control: &Control) -> Result<()> {
    control.check()?;
    ensure!(
        super::package::scan(root)? == entry.sources,
        "Launcher source cache changed; reinstall its original archive"
    );
    package::validate_payloads(root, &entry.files, &entry.sources)
}

#[cfg(test)]
pub(super) use package::parse;
#[cfg(test)]
pub(super) use service::apply_in;

#[cfg(test)]
mod tests;
