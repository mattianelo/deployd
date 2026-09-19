use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::Target;
use super::package::{FileMapping, PackagePlan, SourceFile};

const ANCHOR: &str = "~game~/";
const BIN: &str = "Binaries/Win64/";
const LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Approval {
    target: Target,
    source_sha256: String,
}

impl Approval {
    pub(crate) fn for_plan(plan: &PackagePlan) -> Self {
        Self {
            target: plan.manifest.target,
            source_sha256: plan.source_sha256.clone(),
        }
    }

    pub(crate) fn matches(&self, plan: &PackagePlan) -> bool {
        self.target == plan.manifest.target && self.source_sha256 == plan.source_sha256
    }

    pub(super) fn validate(&self, target: Target, hash: &str) -> Result<()> {
        ensure!(
            self.target == target && self.source_sha256 == hash,
            "Binary-mod approval belongs to another source or game; approve this package again"
        );
        Ok(())
    }
}

pub(super) fn executable(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("asi") || ext.eq_ignore_ascii_case("dll"))
}

pub(super) fn source_name(path: &str) -> Result<()> {
    if !executable(path) {
        super::package::validate_source_name(path)?;
    }
    Ok(())
}

pub(super) fn destination(path: &str) -> Result<()> {
    super::baseline::relative(path)?;
    let name = path
        .strip_prefix(BIN)
        .context("Binary mods require the selected game's Binaries/Win64 folder")?;
    ensure!(
        !name.is_empty() && !super::components::reserved(path),
        "This file is reserved for Deployd's required runtime components"
    );
    let ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        ["asi", "dll", "ini", "json", "toml", "xml", "txt", "cfg"].contains(&ext.as_str()),
        "Unsupported binary-mod file; installers and scripts cannot be deployed"
    );
    if ext == "asi" {
        ensure!(
            name.strip_prefix("ASI/")
                .is_some_and(|name| !name.contains('/')),
            "ASI plugins must be directly inside Binaries/Win64/ASI"
        );
    }
    Ok(())
}

pub(super) fn encode(path: &str) -> Result<String> {
    destination(path)?;
    Ok(format!("{ANCHOR}{path}"))
}

pub(super) fn game_path(path: &str) -> Result<String> {
    if let Some(path) = path.strip_prefix(ANCHOR) {
        destination(path)?;
        Ok(path.into())
    } else {
        let path = format!("BioGame/{path}");
        super::journal::destination(&path)?;
        Ok(path)
    }
}

pub(super) fn mapping(file: &FileMapping) -> Result<()> {
    let path = game_path(&file.destination)?;
    if executable(&file.source) {
        ensure!(
            path.starts_with(BIN) && executable(&path),
            "ASI/DLL payloads cannot be renamed as content or configuration files"
        );
    }
    Ok(())
}

pub(super) fn root_mapping(path: &str) -> bool {
    path.starts_with(ANCHOR)
}

pub(super) fn needs_approval(plan: &PackagePlan) -> bool {
    plan.sources.iter().any(|file| executable(&file.relative))
        || plan
            .files
            .iter()
            .any(|file| root_mapping(&file.destination))
        || plan.manifest.alternates.iter().any(|alternate| {
            super::alternates::mappings(plan, alternate)
                .map(|files| files.iter().any(|file| root_mapping(&file.destination)))
                .unwrap_or(true)
        })
}

pub(super) fn inspect(root: &Path, sources: &[SourceFile]) -> Result<()> {
    for source in sources.iter().filter(|source| executable(&source.relative)) {
        let data = super::m3za::read_input(root, source, LIMIT)?;
        pe(&data)
            .with_context(|| format!("Invalid Windows x86-64 plugin '{}'", source.relative))?;
    }
    Ok(())
}

pub(super) fn inspect_mappings(root: &Path, plan: &PackagePlan) -> Result<()> {
    let mut files = plan.files.clone();
    for alternate in &plan.manifest.alternates {
        files.extend(super::alternates::mappings(plan, alternate)?);
    }
    let mut checked = std::collections::BTreeSet::new();
    for file in files {
        mapping(&file)?;
        if !root_mapping(&file.destination) {
            continue;
        }
        let source = plan
            .sources
            .iter()
            .find(|source| source.relative == file.source)
            .context("Binary-mod mapping has no source")?;
        ensure!(
            source.size <= LIMIT as u64,
            "Binary-mod file exceeds the 64 MiB limit"
        );
        if executable(&file.destination)
            && !executable(&file.source)
            && checked.insert(file.source.clone())
        {
            let data = super::m3za::read_input(root, source, LIMIT)?;
            pe(&data)
                .with_context(|| format!("Invalid Windows x86-64 plugin '{}'", file.source))?;
        }
    }
    Ok(())
}

pub(super) fn pe(data: &[u8]) -> Result<()> {
    let word = |offset: usize| -> Result<u16> {
        Ok(u16::from_le_bytes(
            data.get(offset..offset + 2)
                .context("Truncated PE header")?
                .try_into()?,
        ))
    };
    let dword = |offset: usize| -> Result<usize> {
        Ok(u32::from_le_bytes(
            data.get(offset..offset + 4)
                .context("Truncated PE header")?
                .try_into()?,
        ) as usize)
    };
    ensure!(
        data.len() >= 64 && data.len() <= LIMIT && &data[..2] == b"MZ",
        "Missing DOS header"
    );
    let header = dword(60)?;
    ensure!(
        header >= 64 && data.get(header..header + 4) == Some(b"PE\0\0"),
        "Missing PE signature"
    );
    let sections = usize::from(word(header + 6)?);
    let optional = usize::from(word(header + 20)?);
    ensure!(
        word(header + 4)? == 0x8664
            && word(header + 22)? & 0x2002 == 0x2002
            && word(header + 24)? == 0x20b
            && optional >= 112
            && (1..=96).contains(&sections),
        "Plugins must be executable PE32+ x86-64 DLLs"
    );
    let table = header + 24 + optional;
    ensure!(
        table + sections * 40 <= data.len(),
        "Truncated PE section table"
    );
    for index in 0..sections {
        let entry = table + index * 40;
        let size = dword(entry + 16)?;
        let offset = dword(entry + 20)?;
        ensure!(
            offset <= data.len() && size <= data.len() - offset,
            "PE section lies outside the file"
        );
    }
    Ok(())
}

pub(super) fn package(package: &super::recipe::Package, target: Target) -> Result<()> {
    if let Some(approval) = &package.binary_approval {
        approval.validate(target, &package.source_sha256)?;
    }
    ensure!(
        package.binary_files.is_empty() || package.binary_approval.is_some(),
        "Binary file ownership requires explicit approval"
    );
    let mut paths = BTreeMap::new();
    for file in &package.binary_files {
        destination(&file.relative)?;
        super::journal::Identity {
            size: file.size,
            sha256: file.sha256.clone(),
        }
        .validate()?;
        ensure!(
            file.size <= LIMIT as u64 && paths.insert(file.relative.to_lowercase(), ()).is_none(),
            "Invalid or duplicate binary-mod file"
        );
    }
    Ok(())
}

pub(super) fn inventory(recipe: &super::recipe::Recipe) -> Result<Vec<SourceFile>> {
    let mut files = BTreeMap::new();
    for selected in recipe.packages.iter().filter(|package| package.enabled) {
        package(selected, recipe.target)?;
        for file in &selected.binary_files {
            files.insert(file.relative.to_lowercase(), file.clone());
        }
    }
    Ok(files.into_values().collect())
}

pub(super) fn owned(state: &super::journal::State) -> Result<Vec<SourceFile>> {
    let files = state
        .recipe
        .as_ref()
        .map(inventory)
        .transpose()?
        .unwrap_or_default();
    ensure!(
        files.iter().all(|file| state.files.contains(file)),
        "Binary-mod ownership differs from the deployed file inventory"
    );
    Ok(files)
}

#[cfg(test)]
pub(super) mod tests;
