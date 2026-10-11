use super::*;

pub(crate) fn generated(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    matches!(parts.as_slice(), ["BioGame", "PCConsoleTOC.bin"])
        || matches!(parts.as_slice(), ["BioGame", "DLC", name, "PCConsoleTOC.bin"]
            if name.starts_with("DLC_") && !name.contains(['\\', ':']))
}

pub(super) async fn capture(
    root: PathBuf,
    previous: Option<State>,
    control: Control,
) -> Result<BTreeMap<String, Identity>> {
    tokio::task::spawn_blocking(move || {
        let mut changed = BTreeMap::new();
        for file in previous.iter().flat_map(|state| &state.files) {
            if !generated(&file.relative) {
                continue;
            }
            let identity = files::inspect(&root, &file.relative, &control)?;
            if identity.size != file.size || identity.sha256 != file.sha256 {
                changed.insert(file.relative.clone(), identity);
            }
        }
        Ok(changed)
    })
    .await
    .context("MELE generated-index inspection worker stopped")?
}

pub(super) fn observed(
    previous: Option<&State>,
    indexes: &BTreeMap<String, Identity>,
) -> Result<Option<State>> {
    let mut previous = previous.cloned();
    for (path, identity) in indexes {
        ensure!(
            generated(path),
            "MELE runtime index escapes its generated path"
        );
        identity.validate()?;
        let file = previous
            .as_mut()
            .and_then(|state| state.files.iter_mut().find(|file| &file.relative == path))
            .context("Only previously deployed MELE indexes can be reconciled automatically")?;
        file.size = identity.size;
        file.sha256 = identity.sha256.clone();
    }
    Ok(previous)
}

impl Journal {
    pub(super) fn observed_previous(&self) -> Result<Option<State>> {
        ensure!(
            self.indexes.is_empty() || self.version >= 7,
            "Generated-index recovery requires MELE journal version 7"
        );
        observed(self.previous.as_ref(), &self.indexes)
    }
}
