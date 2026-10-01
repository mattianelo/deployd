use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, ensure};

use super::helper::ValidatedOutput;
use super::journal::{Control, Identity, files};

#[derive(Clone)]
struct Reference {
    root: PathBuf,
    relative: String,
    identity: Identity,
}

#[derive(Clone)]
pub(crate) struct Sources {
    root: PathBuf,
    references: BTreeMap<String, Reference>,
    names: BTreeMap<String, (String, bool)>,
    outputs: Vec<Arc<ValidatedOutput>>,
    directories: Vec<Arc<tempfile::TempDir>>,
}

impl From<PathBuf> for Sources {
    fn from(root: PathBuf) -> Self {
        Self {
            root,
            references: BTreeMap::new(),
            names: BTreeMap::new(),
            outputs: Vec::new(),
            directories: Vec::new(),
        }
    }
}

impl From<&Path> for Sources {
    fn from(root: &Path) -> Self {
        root.to_path_buf().into()
    }
}

impl AsRef<Path> for Sources {
    fn as_ref(&self) -> &Path {
        &self.root
    }
}

impl std::ops::Deref for Sources {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.root
    }
}

impl Sources {
    pub(super) fn is_referenced(&self, path: &str) -> bool {
        self.references.contains_key(path)
    }

    pub(super) fn subtree(&self, prefix: &str) -> Self {
        let prefix = format!("{prefix}/");
        Self {
            root: self.root.join(prefix.trim_end_matches('/')),
            references: self
                .references
                .iter()
                .filter_map(|(path, reference)| {
                    path.strip_prefix(&prefix)
                        .map(|path| (path.to_owned(), reference.clone()))
                })
                .collect(),
            names: self
                .names
                .iter()
                .filter_map(|(folded, (path, file))| {
                    Some((
                        folded.strip_prefix(&prefix.to_lowercase())?.to_owned(),
                        (path.strip_prefix(&prefix)?.to_owned(), *file),
                    ))
                })
                .collect(),
            outputs: self.outputs.clone(),
            directories: self.directories.clone(),
        }
    }

    pub(super) fn location<'a>(&'a self, relative: &'a str) -> (&'a Path, &'a str) {
        match self.references.get(relative) {
            Some(reference) => (&reference.root, &reference.relative),
            None => (&self.root, relative),
        }
    }

    pub(super) fn resolve(&self, relative: &str) -> PathBuf {
        let (root, path) = self.location(relative);
        root.join(path)
    }

    pub(super) fn verify(
        &self,
        path: &str,
        expected: Option<&Identity>,
        control: &Control,
    ) -> Result<()> {
        if let Some(reference) = self.references.get(path) {
            ensure!(
                expected == Some(&reference.identity),
                "Candidate reference differs from its planned identity"
            );
            files::verify(&self.root, path, None, control)?;
        }
        let (root, path) = self.location(path);
        files::verify(root, path, expected, control)
    }

    pub(super) fn insert(
        &mut self,
        path: String,
        root: PathBuf,
        relative: String,
        identity: Identity,
        control: &Control,
    ) -> Result<()> {
        files::verify(&self.root, &path, None, control)?;
        ensure!(
            !self.references.contains_key(&path),
            "Candidate output was not removed before replacement"
        );
        files::verify(&root, &relative, Some(&identity), control)?;
        let mut prefix = String::new();
        let parts: Vec<_> = path.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let is_file = index + 1 == parts.len();
            if let Some((existing, file)) = self.names.get(&prefix.to_lowercase()) {
                ensure!(
                    existing == &prefix && *file == is_file,
                    "Case-colliding or overlapping candidate path"
                );
            }
            self.names
                .insert(prefix.to_lowercase(), (prefix.clone(), is_file));
        }
        self.references.insert(
            path,
            Reference {
                root,
                relative,
                identity,
            },
        );
        Ok(())
    }

    pub(super) fn remove(
        &mut self,
        path: &str,
        expected: Option<&Identity>,
        control: &Control,
    ) -> Result<()> {
        self.verify(path, expected, control)?;
        self.names.remove(&path.to_lowercase());
        if self.references.remove(path).is_none() && expected.is_some() {
            std::fs::remove_file(self.root.join(path))?;
        }
        Ok(())
    }

    pub(super) fn materialize(&mut self, path: &str, control: &Control) -> Result<()> {
        let Some(reference) = self.references.get(path) else {
            return control.check();
        };
        files::copy(
            &reference.root,
            &reference.relative,
            &self.root,
            path,
            &reference.identity,
            control,
        )?;
        self.references.remove(path);
        Ok(())
    }

    pub(super) fn keep_directory(&mut self, directory: tempfile::TempDir) {
        self.directories.push(Arc::new(directory));
    }

    pub(super) fn cache_keys(&self) -> std::collections::BTreeSet<String> {
        self.outputs
            .iter()
            .filter_map(|output| output.cache_key.clone())
            .collect()
    }

    pub(super) fn keep(&mut self, output: ValidatedOutput) {
        self.outputs.push(Arc::new(output));
    }
}

#[cfg(test)]
mod tests;
