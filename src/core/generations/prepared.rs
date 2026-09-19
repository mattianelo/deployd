use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use crate::core::game::engine_handler::handler_for;
use crate::models::game::GameEngine;

use super::manifest::{Manifest, Output};
use super::records::{Table, text};
use super::target::Target;

pub(super) fn files(manifest: &Manifest) -> Result<Vec<Output>> {
    manifest.validate()?;
    ensure!(
        manifest.engine != GameEngine::MassEffect,
        "MELE outputs require its recipe preparation participant"
    );
    let handler = handler_for(&manifest.engine);
    let mods = manifest
        .records
        .iter()
        .find(|rows| rows.table == Table::ProfileMods)
        .context("Historical mod ordering is absent")?;
    let mut priorities = BTreeMap::new();
    for row in &mods.rows {
        if row.get("enabled").and_then(serde_json::Value::as_i64) == Some(1) {
            let priority = row
                .get("priority")
                .and_then(serde_json::Value::as_i64)
                .context("Historical mod priority is invalid")?;
            priorities.insert(text(row, "mod_id")?, priority);
        }
    }
    let sources: BTreeMap<_, _> = manifest
        .sources
        .iter()
        .map(|source| (source.path.as_str(), source))
        .collect();
    let files = manifest
        .records
        .iter()
        .find(|rows| rows.table == Table::Files)
        .context("Historical file routing is absent")?;
    let mut winners = BTreeMap::new();
    for row in &files.rows {
        let mod_id = text(row, "mod_id")?;
        let Some(priority) = priorities.get(mod_id) else {
            continue;
        };
        let lower = text(row, "game_rel_lowercase")?;
        let original = text(row, "game_rel_original")?;
        ensure!(
            original.to_lowercase() == lower,
            "Historical routing has inconsistent path identities"
        );
        let key = handler.conflict_key(lower).to_owned();
        let rank = (
            *priority,
            std::cmp::Reverse(lower),
            std::cmp::Reverse(mod_id),
        );
        let source = sources
            .get(text(row, "cache_path")?)
            .context("Historical routing references missing content")?;
        ensure!(
            source.content.is_none() == original.ends_with('/'),
            "Historical file and directory routing disagree"
        );
        let output = Output {
            target: Target::file(&manifest.engine, original)?,
            content: source.content.clone(),
            mode: source.mode,
            mod_id: Some(mod_id.to_owned()),
        };
        match winners.get(&key) {
            Some((previous, _)) if previous >= &rank => {}
            _ => {
                winners.insert(key, (rank, output));
            }
        }
    }
    let mut outputs: Vec<_> = winners.into_values().map(|(_, output)| output).collect();
    outputs.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(outputs)
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(super) enum Difference {
    Added(Target),
    Removed(Target),
    Changed(Target),
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn compare(previous: &[Output], next: &[Output]) -> Result<Vec<Difference>> {
    let index =
        |outputs: &[Output]| -> Result<BTreeMap<Target, (Option<super::content::Identity>, u32)>> {
            let mut entries = BTreeMap::new();
            for output in outputs {
                ensure!(
                    entries
                        .insert(output.target.clone(), (output.content.clone(), output.mode))
                        .is_none(),
                    "Duplicate deployment target prevents comparison"
                );
            }
            Ok(entries)
        };
    let previous = index(previous)?;
    let next = index(next)?;
    let mut differences = Vec::new();
    for (target, identity) in &previous {
        match next.get(target) {
            None => differences.push(Difference::Removed(target.clone())),
            Some(updated) if updated != identity => {
                differences.push(Difference::Changed(target.clone()))
            }
            _ => {}
        }
    }
    for target in next.keys().filter(|target| !previous.contains_key(*target)) {
        differences.push(Difference::Added(target.clone()));
    }
    Ok(differences)
}
