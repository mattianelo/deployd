use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use crate::models::game::GameEngine;

use super::manifest::Manifest;
use super::records::{Record, Table, text};
use super::target::{Target, relative};

fn identity(row: &Record, column: &str) -> Result<String> {
    let id = text(row, column)?;
    relative(id)?;
    ensure!(
        !id.contains('/'),
        "Historical identity contains a path separator"
    );
    Ok(id.to_owned())
}

pub(super) fn metadata(manifest: &Manifest) -> Result<()> {
    let rows = |table| {
        manifest
            .records
            .iter()
            .find(|rows| rows.table == table)
            .map(|rows| rows.rows.as_slice())
            .context("Incomplete historical tables")
    };
    let ids = |table| -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for row in rows(table)? {
            ensure!(
                ids.insert(identity(row, "id")?),
                "Duplicate historical identity"
            );
        }
        Ok(ids)
    };
    let mods = ids(Table::Mods)?;
    let plugins = ids(Table::Plugins)?;
    let groups = ids(Table::Groups)?;
    let (profile, _) = manifest.profile()?;
    for table in [Table::Mods, Table::Groups, Table::MeleRecipes] {
        for row in rows(table)? {
            ensure!(
                text(row, "game_id")? == manifest.game_id,
                "Historical metadata belongs to another game"
            );
        }
    }
    for row in rows(Table::Mods)? {
        if let Some(group) = row.get("group_id").filter(|value| !value.is_null()) {
            ensure!(
                group.as_str().is_some_and(|group| groups.contains(group)),
                "Historical mod group is missing"
            );
        }
    }
    for table in [
        Table::Files,
        Table::Plugins,
        Table::MelePackages,
        Table::ProfileMods,
    ] {
        for row in rows(table)? {
            ensure!(
                mods.contains(text(row, "mod_id")?),
                "Historical mod reference is missing"
            );
        }
    }
    for table in [Table::Masters, Table::ProfilePlugins] {
        for row in rows(table)? {
            ensure!(
                plugins.contains(text(row, "plugin_id")?),
                "Historical plugin reference is missing"
            );
        }
    }
    for table in [
        Table::ProfileMods,
        Table::ProfilePlugins,
        Table::MeleRecipes,
    ] {
        for row in rows(table)? {
            ensure!(
                text(row, "profile_id")? == profile,
                "Historical configuration belongs to another profile"
            );
        }
    }
    for (table, column, expected) in [
        (Table::ProfileMods, "mod_id", &mods),
        (Table::ProfilePlugins, "plugin_id", &plugins),
    ] {
        let mut actual = BTreeSet::new();
        for row in rows(table)? {
            ensure!(
                actual.insert(identity(row, column)?),
                "Duplicate historical profile membership"
            );
        }
        ensure!(
            &actual == expected,
            "Historical profile ordering is incomplete"
        );
    }
    let mut roots = BTreeSet::new();
    if manifest.engine == GameEngine::MassEffect {
        let mut packages = BTreeSet::new();
        for row in rows(Table::MelePackages)? {
            let record: crate::core::game::mass_effect::library::Record =
                serde_json::from_str(text(row, "document")?)?;
            record.validate()?;
            ensure!(
                record.package.id == text(row, "mod_id")?,
                "Historical MELE package identity differs"
            );
            ensure!(
                packages.insert(record.package.id),
                "Duplicate historical MELE package"
            );
            roots.insert(format!("mele-sources/{}", record.package.source_sha256));
        }
        ensure!(
            packages == mods,
            "Historical MELE source inventory is incomplete"
        );
    } else {
        ensure!(
            rows(Table::MelePackages)?.is_empty() && rows(Table::MeleRecipes)?.is_empty(),
            "MELE metadata belongs to another engine"
        );
        roots.extend(mods.iter().map(|id| format!("cache/{id}")));
    }
    let sources: BTreeMap<_, _> = manifest
        .sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect();
    for root in &roots {
        ensure!(
            sources
                .get(root.as_str())
                .is_some_and(|source| source.content.is_none()),
            "Historical mod root is missing"
        );
    }
    for source in &manifest.sources {
        let mut parts = source.path.split('/');
        let root = format!(
            "{}/{}",
            parts.next().context("Missing historical source anchor")?,
            parts.next().context("Missing historical source identity")?
        );
        ensure!(
            roots.contains(&root),
            "Historical content is outside its installed mod inventory"
        );
    }
    let mut routes = BTreeSet::new();
    for row in rows(Table::Files)? {
        let mod_id = text(row, "mod_id")?;
        let original = text(row, "game_rel_original")?;
        let lower = text(row, "game_rel_lowercase")?;
        Target::file(&manifest.engine, original)?;
        ensure!(
            original.to_lowercase() == lower,
            "Historical routing has inconsistent path identities"
        );
        ensure!(
            routes.insert((mod_id, lower)),
            "Duplicate historical file routing"
        );
        let logical = text(row, "cache_path")?;
        let source = sources
            .get(logical)
            .context("Historical routing references missing source content")?;
        ensure!(
            source.content.is_none() == original.ends_with('/'),
            "Historical file and directory routing disagree"
        );
        if manifest.engine != GameEngine::MassEffect {
            let root = format!("cache/{mod_id}");
            ensure!(
                logical == root || logical.starts_with(&format!("{root}/")),
                "Historical file references another mod's inventory"
            );
        }
    }
    for output in &manifest.outputs {
        if let Some(id) = &output.mod_id {
            ensure!(
                mods.contains(id),
                "Historical output references a missing mod"
            );
        }
    }
    Ok(())
}
