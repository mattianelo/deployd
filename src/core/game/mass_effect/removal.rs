use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use super::Target;
use super::baseline::{self, Baseline};
use super::journal::{Identity, State, files};
use super::operation::Control;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Removals {
    pub(crate) paths: BTreeSet<String>,
    pub(crate) dlc: BTreeSet<String>,
}

impl Removals {
    pub(crate) fn is_empty(&self) -> bool {
        self.paths.is_empty() && self.dlc.is_empty()
    }

    pub(super) fn validate(&self, target: Target) -> Result<()> {
        ensure!(
            self.paths.len() <= 100_000 && self.dlc.len() <= 1024,
            "MELE removal plan exceeds its limits"
        );
        let mut folded = BTreeSet::new();
        for name in &self.dlc {
            super::manifest::validate_dlc(name)?;
            ensure!(
                !target.is_official_dlc(name) && folded.insert(name.to_lowercase()),
                "Invalid or duplicate obsolete DLC '{name}'"
            );
        }
        folded.clear();
        for path in &self.paths {
            super::journal::destination(path)?;
            ensure!(
                dlc(path).is_some_and(|name| self.dlc.contains(name))
                    && folded.insert(path.to_lowercase()),
                "Removal path is outside its obsolete DLC or is duplicated"
            );
        }
        Ok(())
    }

    pub(super) fn retire(
        &mut self,
        names: &[String],
        available: &mut BTreeSet<String>,
        control: &Control,
    ) -> Result<Vec<String>> {
        let mut removed = Vec::new();
        for name in names {
            control.check()?;
            ensure!(
                !name.eq_ignore_ascii_case(super::merge_dlc::NAME),
                "Archives cannot retire the reserved generated merge DLC"
            );
            let matching: Vec<_> = available
                .iter()
                .filter(|path| dlc(path).is_some_and(|folder| folder.eq_ignore_ascii_case(name)))
                .cloned()
                .collect();
            let folders: BTreeSet<_> = matching.iter().filter_map(|path| dlc(path)).collect();
            ensure!(folders.len() <= 1, "Case-colliding obsolete DLC '{name}'");
            let folder = folders
                .first()
                .copied()
                .or_else(|| {
                    self.dlc
                        .iter()
                        .find(|existing| existing.eq_ignore_ascii_case(name))
                        .map(String::as_str)
                })
                .unwrap_or(name)
                .to_string();
            self.dlc
                .retain(|existing| !existing.eq_ignore_ascii_case(&folder));
            self.dlc.insert(folder);
            ensure!(self.dlc.len() <= 1024, "Too many obsolete DLC declarations");
            for path in matching {
                control.check()?;
                available.remove(&path);
                self.paths.insert(path.clone());
                removed.push(path);
            }
            ensure!(self.paths.len() <= 100_000, "Too many obsolete DLC files");
        }
        Ok(removed)
    }
}

pub(super) fn dlc(path: &str) -> Option<&str> {
    path.strip_prefix("BioGame/DLC/")?
        .split_once('/')
        .map(|(name, _)| name)
}

fn expected(baseline: &Baseline, state: Option<&State>) -> BTreeMap<String, Identity> {
    let mut expected: BTreeMap<_, _> = baseline
        .files
        .iter()
        .map(|file| {
            (
                file.relative.clone(),
                Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
            )
        })
        .collect();
    if let Some(state) = state {
        expected.extend(state.files.iter().map(|file| {
            (
                file.relative.clone(),
                Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
            )
        }));
        for path in &state.removals.paths {
            expected.remove(path);
        }
    }
    expected
}

fn roots(game: &Path, scopes: &BTreeSet<String>, control: &Control) -> Result<Vec<PathBuf>> {
    if scopes.is_empty() {
        return Ok(Vec::new());
    }
    baseline::directory(game)?;
    let mut root = game.to_path_buf();
    for part in ["BioGame", "DLC"] {
        for (index, entry) in fs::read_dir(&root)?.enumerate() {
            control.check()?;
            ensure!(index < 100_000, "DLC parent inventory exceeds its limit");
            let name = entry?.file_name();
            let name = name.to_str().context("Invalid DLC parent name")?;
            ensure!(
                !name.eq_ignore_ascii_case(part) || name == part,
                "Case-colliding DLC parent directory"
            );
        }
        root.push(part);
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).context("Cannot inspect DLC; restore folder access");
            }
            Ok(_) => baseline::directory(&root)?,
        }
    }
    let mut result = Vec::new();
    for (index, entry) in fs::read_dir(root)?.enumerate() {
        control.check()?;
        ensure!(index < 100_000, "DLC inventory exceeds its limit");
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("Invalid DLC folder name")?;
        if scopes.iter().any(|scope| scope.eq_ignore_ascii_case(name)) {
            baseline::directory(&entry.path())?;
            result.push(entry.path());
        }
    }
    Ok(result)
}

pub(super) fn scopes(previous: Option<&State>, desired: &State) -> BTreeSet<String> {
    previous
        .into_iter()
        .flat_map(|state| &state.removals.dlc)
        .chain(&desired.removals.dlc)
        .cloned()
        .chain(std::iter::once(super::merge_dlc::NAME.to_owned()))
        .collect()
}

pub(super) fn verify(
    game: &Path,
    baseline: &Baseline,
    state: Option<&State>,
    scopes: &BTreeSet<String>,
    control: &Control,
) -> Result<()> {
    super::merge_dlc::baseline(baseline)?;
    let mut scopes = scopes.clone();
    scopes.insert(super::merge_dlc::NAME.to_owned());
    let expected = expected(baseline, state);
    let mut count = 0;
    for root in roots(game, &scopes, control)? {
        for entry in WalkDir::new(root).follow_links(false) {
            control.check()?;
            count += 1;
            ensure!(count <= 200_000, "DLC inventory exceeds its limit");
            let entry = entry.context("Cannot inspect DLC; restore folder access")?;
            if entry.file_type().is_dir() {
                continue;
            }
            let path = entry
                .path()
                .strip_prefix(game)?
                .to_str()
                .context("Invalid DLC path")?;
            let identity = expected.get(path).with_context(|| format!(
                "Unmanaged file in protected DLC '{path}'; preserve it and reconcile before deploying"))?;
            files::verify(game, path, Some(identity), control)?;
        }
    }
    if let Some(state) = state {
        for path in &state.removals.paths {
            files::verify(game, path, None, control)?;
        }
    }
    Ok(())
}

pub(super) fn cleanup(
    game: &Path,
    baseline: &Baseline,
    state: &State,
    control: &Control,
) -> Result<()> {
    verify(game, baseline, Some(state), &state.removals.dlc, control)?;
    let mut retained: Vec<_> = state
        .files
        .iter()
        .map(|file| file.relative.as_str())
        .collect();
    retained.sort_unstable();
    let mut scopes = state.removals.dlc.clone();
    scopes.insert(super::merge_dlc::NAME.to_owned());
    for root in roots(game, &scopes, control)? {
        for entry in WalkDir::new(root).follow_links(false).contents_first(true) {
            control.check()?;
            let entry = entry?;
            if !entry.file_type().is_dir() {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(game)?
                .to_str()
                .context("Invalid obsolete DLC directory")?;
            let prefix = format!("{relative}/");
            let index = retained.partition_point(|path| *path < prefix.as_str());
            if retained
                .get(index)
                .is_some_and(|path| path.starts_with(&prefix))
            {
                continue;
            }
            baseline::directory(entry.path())?;
            fs::remove_dir(entry.path()).context(
                "Obsolete DLC changed during cleanup; preserve unexpected files and retry recovery",
            )?;
            files::sync(
                entry
                    .path()
                    .parent()
                    .context("Missing obsolete DLC parent")?,
            )?;
        }
    }
    Ok(())
}
