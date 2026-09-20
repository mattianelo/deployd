use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::journal::{Identity, files};
use super::operation::Control;
use super::package::SourceFile;

mod package;
mod reconcile;
pub(crate) use package::inspect_bundle;
pub(crate) use reconcile::reconcile;

#[derive(Debug)]
pub(crate) struct Bundled {
    pub(crate) entry: Entry,
    pub(crate) source: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(super) owner: String,
    pub(crate) name: String,
    pub(super) source_sha256: String,
    pub(super) approval: String,
    pub(super) files: Vec<Mapping>,
    pub(super) sources: Vec<SourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LegacyEntry {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) enabled: bool,
    pub(super) source_sha256: String,
    pub(super) approval: String,
    pub(super) files: Vec<Mapping>,
    pub(super) sources: Vec<SourceFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum PersistedEntry {
    Owned(Entry),
    Legacy(LegacyEntry),
}

impl PersistedEntry {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Owned(entry) => &entry.id,
            Self::Legacy(entry) => &entry.id,
        }
    }

    pub(super) fn owner(&self) -> Option<&str> {
        match self {
            Self::Owned(entry) => Some(&entry.owner),
            Self::Legacy(_) => None,
        }
    }

    pub(super) fn active(&self) -> bool {
        match self {
            Self::Owned(_) => true,
            Self::Legacy(entry) => entry.enabled,
        }
    }

    pub(super) fn source_sha256(&self) -> &str {
        match self {
            Self::Owned(entry) => &entry.source_sha256,
            Self::Legacy(entry) => &entry.source_sha256,
        }
    }

    pub(super) fn files(&self) -> &[Mapping] {
        match self {
            Self::Owned(entry) => &entry.files,
            Self::Legacy(entry) => &entry.files,
        }
    }

    pub(super) fn sources(&self) -> &[SourceFile] {
        match self {
            Self::Owned(entry) => &entry.sources,
            Self::Legacy(entry) => &entry.sources,
        }
    }

    pub(super) fn as_owned(&self) -> Option<&Entry> {
        match self {
            Self::Owned(entry) => Some(entry),
            Self::Legacy(_) => None,
        }
    }

    pub(super) fn matches(&self, entry: &Entry) -> bool {
        self.source_sha256() == entry.source_sha256
            && self.files() == entry.files
            && self.sources() == entry.sources
    }

    pub(super) fn owned(&self, id: String, owner: String) -> Entry {
        match self {
            Self::Owned(entry) => {
                let mut entry = entry.clone();
                entry.id = id;
                entry.owner = owner;
                entry
            }
            Self::Legacy(entry) => Entry {
                id,
                owner,
                name: entry.name.clone(),
                source_sha256: entry.source_sha256.clone(),
                approval: entry.approval.clone(),
                files: entry.files.clone(),
                sources: entry.sources.clone(),
            },
        }
    }
}

impl From<Entry> for PersistedEntry {
    fn from(entry: Entry) -> Self {
        Self::Owned(entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Mapping {
    pub(super) source: String,
    pub(super) destination: String,
    pub(super) identity: Identity,
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
    validate(entries.iter().map(|entry| EntryRef {
        id: &entry.id,
        owner: Some(&entry.owner),
        name: &entry.name,
        source_sha256: &entry.source_sha256,
        approval: &entry.approval,
        files: &entry.files,
        sources: &entry.sources,
    }))
}

pub(super) fn validate_persisted_entries(entries: &[PersistedEntry]) -> Result<()> {
    validate(entries.iter().map(|entry| match entry {
        PersistedEntry::Owned(entry) => EntryRef {
            id: &entry.id,
            owner: Some(&entry.owner),
            name: &entry.name,
            source_sha256: &entry.source_sha256,
            approval: &entry.approval,
            files: &entry.files,
            sources: &entry.sources,
        },
        PersistedEntry::Legacy(entry) => EntryRef {
            id: &entry.id,
            owner: None,
            name: &entry.name,
            source_sha256: &entry.source_sha256,
            approval: &entry.approval,
            files: &entry.files,
            sources: &entry.sources,
        },
    }))
}

struct EntryRef<'a> {
    id: &'a str,
    owner: Option<&'a str>,
    name: &'a str,
    source_sha256: &'a str,
    approval: &'a str,
    files: &'a [Mapping],
    sources: &'a [SourceFile],
}

fn validate<'a>(entries: impl Iterator<Item = EntryRef<'a>>) -> Result<()> {
    let entries: Vec<_> = entries.collect();
    ensure!(entries.len() <= 256, "Too many shared launcher mods");
    let mut ids = BTreeSet::new();
    let mut destinations = BTreeMap::new();
    for entry in entries {
        ensure!(
            Uuid::parse_str(entry.id)?.to_string() == entry.id
                && ids.insert(entry.id)
                && entry.owner.is_none_or(|owner| [
                    "mass-effect-le1",
                    "mass-effect-le2",
                    "mass-effect-le3"
                ]
                .contains(&owner)),
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
                && entry.source_sha256 == super::package::tree_digest(entry.sources),
            "Launcher mod requires approval for this exact source"
        );
        let mut sources = BTreeSet::new();
        for file in entry.sources {
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
        for file in entry.files {
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

pub(super) fn retain(data: &Path, source: &Path, entry: &Entry, control: &Control) -> Result<()> {
    validate_entries(std::slice::from_ref(entry))?;
    retain_payload(
        data,
        source,
        &entry.source_sha256,
        &entry.files,
        &entry.sources,
        control,
    )
}

pub(super) fn retain_persisted(
    data: &Path,
    source: &Path,
    entry: &PersistedEntry,
    control: &Control,
) -> Result<()> {
    validate_persisted_entries(std::slice::from_ref(entry))?;
    retain_payload(
        data,
        source,
        entry.source_sha256(),
        entry.files(),
        entry.sources(),
        control,
    )
}

fn retain_payload(
    data: &Path,
    source: &Path,
    source_sha256: &str,
    mappings: &[Mapping],
    sources: &[SourceFile],
    control: &Control,
) -> Result<()> {
    let cache = data.join("mele-launcher-sources");
    files::create_directory(&cache)?;
    let destination = source_root(data, source_sha256);
    if destination.try_exists()? {
        return verify_payload(&destination, mappings, sources, control);
    }
    let temp = tempfile::Builder::new()
        .prefix("incoming-")
        .tempdir_in(&cache)?;
    for file in sources {
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
    verify_payload(temp.path(), mappings, sources, control)?;
    files::sync(temp.path())?;
    std::fs::rename(temp.path(), &destination)?;
    files::sync(&cache)
}

#[cfg(test)]
pub(super) fn verify_source(root: &Path, entry: &Entry, control: &Control) -> Result<()> {
    verify_payload(root, &entry.files, &entry.sources, control)
}

pub(super) fn verify_persisted_source(
    root: &Path,
    entry: &PersistedEntry,
    control: &Control,
) -> Result<()> {
    verify_payload(root, entry.files(), entry.sources(), control)
}

fn verify_payload(
    root: &Path,
    mappings: &[Mapping],
    sources: &[SourceFile],
    control: &Control,
) -> Result<()> {
    control.check()?;
    ensure!(
        super::package::scan(root)? == sources,
        "Launcher source cache changed; reinstall its original archive"
    );
    package::validate_payloads(root, mappings, sources)
}

#[cfg(test)]
pub(super) use package::parse;
#[cfg(test)]
mod tests;
