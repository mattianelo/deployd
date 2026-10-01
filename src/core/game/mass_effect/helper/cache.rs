use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Backend, Control, FileIdentity, Inputs, Job, ValidatedOutput, files, protocol};
use crate::core::game::mass_effect::{journal, recipe::Recipe};
use crate::utils::paths;

const VERSION: u32 = 1;
const LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Ticket {
    root: PathBuf,
    key: String,
}

pub(super) struct Pin(PathBuf);

fn pins() -> &'static Mutex<BTreeMap<PathBuf, usize>> {
    static PINS: OnceLock<Mutex<BTreeMap<PathBuf, usize>>> = OnceLock::new();
    PINS.get_or_init(Mutex::default)
}

impl Pin {
    fn acquire(path: PathBuf) -> Result<Arc<Self>> {
        *pins()
            .lock()
            .map_err(|_| anyhow::anyhow!("Result cache is unavailable"))?
            .entry(path.clone())
            .or_default() += 1;
        Ok(Arc::new(Self(path)))
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        if let Ok(mut pins) = pins().lock()
            && let Some(count) = pins.get_mut(&self.0)
        {
            *count -= 1;
            if *count == 0 {
                pins.remove(&self.0);
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    key: String,
    files: Vec<FileIdentity>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Usage {
    version: u32,
    recipes: BTreeMap<String, BTreeSet<String>>,
    profiles: BTreeMap<String, Vec<String>>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn identity(path: &Path, control: &Control) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_file(), "Cache dependency is not a regular file");
    let root = path.parent().context("Missing dependency parent")?;
    files::directory(root)?;
    let shared = crate::core::generations::content::Control {
        cancelled: control.cancelled.clone(),
        ..Default::default()
    };
    control.check()?;
    let identity = crate::core::generations::content::inspect(path, &shared)?;
    control.check()?;
    Ok(format!("{}:{}", identity.size, identity.sha256))
}

pub(super) fn ticket(
    backend: &Backend,
    inputs: &Inputs,
    parent: &Path,
    job: &Job,
    control: &Control,
) -> Result<Option<Ticket>> {
    let codec = inputs.game.join("Binaries/Win64/oo2core_8_win64.dll");
    if !codec.try_exists()? {
        return Ok(None);
    }
    let mut dependencies = BTreeMap::new();
    for (role, path) in [
        ("selected-runtime", &backend.runtime),
        ("selected-assembly", &backend.assembly),
        ("selected-native", &backend.native_library),
    ] {
        dependencies.insert(role.to_owned(), identity(path, control)?);
    }
    dependencies.insert("game-codec".to_owned(), identity(&codec, control)?);
    for (tag, root) in [
        ("helper", backend.assembly.parent()),
        ("runtime", backend.runtime.parent()),
    ] {
        let root = root.context("Missing backend directory")?;
        files::directory(root)?;
        for entry in walkdir::WalkDir::new(root).follow_links(false).min_depth(1) {
            control.check()?;
            let entry = entry?;
            ensure!(
                entry.file_type().is_dir() || entry.file_type().is_file(),
                "Backend dependency contains a link or special file"
            );
            if entry.file_type().is_file() {
                let path = entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("Invalid backend dependency name")?;
                dependencies.insert(format!("{tag}/{path}"), identity(entry.path(), control)?);
                ensure!(
                    dependencies.len() <= 10000,
                    "Backend dependency inventory exceeds its limit"
                );
            }
        }
    }
    let key = fingerprint(job, &dependencies)?;
    Ok(Some(Ticket {
        root: paths::mele_result_cache_in(
            parent
                .parent()
                .context("Missing transformation storage parent")?,
        ),
        key,
    }))
}

fn fingerprint(job: &Job, dependencies: &BTreeMap<String, String>) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&(
        VERSION,
        protocol::VERSION,
        job.game(),
        job,
        dependencies,
    ))?))
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1 && metadata.len() <= LIMIT,
        "Invalid result cache metadata"
    );
    let mut bytes = Vec::new();
    file.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "Result cache metadata exceeds its limit"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("Missing result cache parent")?;
    files::directory(parent)?;
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() as u64 <= LIMIT,
        "Result cache metadata exceeds its limit"
    );
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

impl Ticket {
    fn directory(&self) -> PathBuf {
        self.root.join("objects").join(&self.key)
    }

    pub(super) fn load(&self, job: &Job, control: &Control) -> Result<ValidatedOutput> {
        control.check()?;
        let directory = self.directory();
        files::directory(&directory)?;
        let manifest: Manifest = read(&directory.join("manifest.json"))?;
        ensure!(
            manifest.version == VERSION && manifest.key == self.key,
            "Incompatible result cache entry"
        );
        files::outputs(
            &directory.join("output"),
            &job.outputs(),
            &manifest.files,
            job.byte_limit(),
            control,
        )?;
        control.check()?;
        Ok(ValidatedOutput {
            storage: super::OutputStorage::Cached {
                root: directory,
                _pin: Pin::acquire(self.directory())?,
            },
            files: manifest.files,
            cache_key: Some(self.key.clone()),
        })
    }

    pub(super) fn publish(
        &self,
        output: &ValidatedOutput,
        job: &Job,
        control: &Control,
    ) -> Result<ValidatedOutput> {
        control.check()?;
        journal::files::create_directory(&self.root.join("objects"))?;
        let temporary = tempfile::Builder::new()
            .prefix(".result-")
            .tempdir_in(&self.root)?;
        let root = temporary.path().join("output");
        journal::files::create_directory(&root)?;
        for file in &output.files {
            journal::files::copy(
                &output.root(),
                &file.path,
                &root,
                &file.path,
                &journal::Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
                control,
            )?;
        }
        write(
            &temporary.path().join("manifest.json"),
            &Manifest {
                version: VERSION,
                key: self.key.clone(),
                files: output.files.clone(),
            },
        )?;
        control.check()?;
        if self.directory().try_exists()? {
            if let Ok(cached) = self.load(job, control) {
                return Ok(cached);
            }
            files::directory(&self.directory())?;
            ensure!(
                !pins()
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Result cache is unavailable"))?
                    .contains_key(&self.directory()),
                "Damaged result cache entry is still in use"
            );
            fs::remove_dir_all(self.directory())?;
        }
        fs::rename(temporary.path(), self.directory())?;
        File::open(self.root.join("objects"))?.sync_all()?;
        self.load(job, control)
    }
}

fn usage(root: &Path) -> Result<Usage> {
    let path = root.join("usage.json");
    if !path.try_exists()? {
        return Ok(Usage {
            version: VERSION,
            ..Default::default()
        });
    }
    let usage: Usage = read(&path)?;
    ensure!(
        usage.version == VERSION
            && usage.recipes.keys().all(|key| valid_key(key))
            && usage.recipes.values().flatten().all(|key| valid_key(key))
            && usage
                .profiles
                .values()
                .all(|recipes| recipes.len() <= 2 && recipes.iter().all(|key| valid_key(key))),
        "Invalid result cache usage"
    );
    Ok(usage)
}

pub(in crate::core::game::mass_effect) fn prepared(
    data: &Path,
    recipe: &Recipe,
    keys: BTreeSet<String>,
) -> Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let root = paths::mele_result_cache_in(data);
    journal::files::create_directory(&root)?;
    let mut usage = usage(&root).unwrap_or_else(|_| Usage {
        version: VERSION,
        ..Default::default()
    });
    usage
        .recipes
        .insert(digest(&serde_json::to_vec(recipe)?), keys);
    write(&root.join("usage.json"), &usage)
}

pub(in crate::core::game::mass_effect) fn committed(
    data: &Path,
    recipe: &Recipe,
    profile: &str,
) -> Result<()> {
    let root = paths::mele_result_cache_in(data);
    if !root.try_exists()? {
        return Ok(());
    }
    files::directory(&root)?;
    let mut usage = usage(&root).unwrap_or_else(|_| Usage {
        version: VERSION,
        ..Default::default()
    });
    let key = digest(&serde_json::to_vec(recipe)?);
    let profile = digest(&serde_json::to_vec(&(recipe.target, profile))?);
    let recipes = usage.profiles.entry(profile).or_default();
    if recipes.first() != Some(&key) {
        recipes.retain(|old| old != &key);
        recipes.insert(0, key);
        recipes.truncate(2);
    }
    let active = pins()
        .lock()
        .map_err(|_| anyhow::anyhow!("Result cache is unavailable"))?;
    let recipes: BTreeSet<_> = usage.profiles.values().flatten().cloned().collect();
    usage.recipes.retain(|recipe, keys| {
        recipes.contains(recipe)
            || keys
                .iter()
                .any(|key| active.contains_key(&root.join("objects").join(key)))
    });
    let retained: BTreeSet<_> = usage.recipes.values().flatten().collect();
    let objects = root.join("objects");
    if objects.try_exists()? {
        files::directory(&objects)?;
        for entry in fs::read_dir(&objects)? {
            let entry = entry?;
            let key = entry.file_name();
            let Some(key) = key.to_str().filter(|key| valid_key(key)) else {
                continue;
            };
            if !retained.contains(&key.to_owned()) && !active.contains_key(&entry.path()) {
                files::directory(&entry.path())?;
                fs::remove_dir_all(entry.path())?;
            }
        }
    }
    for entry in fs::read_dir(&root)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(".result-"))
        {
            files::directory(&entry.path())?;
            fs::remove_dir_all(entry.path())?;
        }
    }
    write(&root.join("usage.json"), &usage)
}

#[cfg(test)]
mod tests;
