use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use crate::core::game;
use crate::models::game::{Game, GameEngine};
use crate::utils::snap::{self, SelectedFolderKind};

use super::catalog::History;
use super::content::{self, Control};
use super::journal::{Change, Journal, Node};
use super::manifest::{Manifest, Output};
use super::records::{Record, Table, text};
use super::target::{Target, relative};

mod ini;

pub(super) struct PreparedIni {
    pub(super) outputs: Vec<Output>,
    pub(super) journal: Journal,
}

pub(super) async fn inis(
    history: &History,
    game: &Game,
    journal: Journal,
    control: Control,
) -> Result<PreparedIni> {
    ensure!(
        history.game == game.id && journal.game == game.id,
        "INI preparation belongs to another game"
    );
    ensure!(
        game.engine == GameEngine::Bethesda,
        "Managed INI preparation belongs to Bethesda"
    );
    ensure!(
        !journal.has_saves(),
        "Prepare managed INIs before preparing save transitions"
    );
    control.check()?;
    history.tracker.ensure_location_ready(&game.id).await?;
    let store = history.store.clone();
    let game = game.clone();
    let preparing = control.clone();
    let (prepared, objects) = history
        .lease
        .blocking(move || -> Result<_> {
            let mut journal = journal;
            journal.validate(&game)?;
            let slots = game::custom_ini_paths(&game).len();
            ensure!(
                slots > 0,
                "Managed INI locations are unavailable; restore Wine-prefix access"
            );
            let mut outputs = Vec::new();
            let mut objects = BTreeMap::new();
            for slot in 0..slots {
                preparing.check()?;
                let target = Target::CustomIni { slot };
                ensure!(
                    !journal.changes.iter().any(|change| change.target == target),
                    "This activation already contains a prepared managed INI"
                );
                let before = super::divergence::live(&game, &target, &preparing)?;
                let mut bytes = Vec::new();
                let mode = match &before {
                    Node::File { identity, mode } => {
                        ensure!(
                            store.retain(&target.resolve(&game)?, &preparing)? == *identity,
                            "Managed INI changed while capturing its original content"
                        );
                        ensure!(
                            content::transfer(&store.source(identity)?, &mut bytes, &preparing)?
                                == *identity,
                            "Retained INI input changed during preparation"
                        );
                        *mode
                    }
                    Node::Absent => 0o644,
                    Node::Directory { .. } => {
                        anyhow::bail!("Managed INI location is a directory; it was preserved")
                    }
                };
                let content = store.retain_generated(&ini::render(&bytes)?, &preparing)?;
                ensure!(
                    super::divergence::live(&game, &target, &preparing)? == before,
                    "Managed INI changed during preparation; prepare deployment again"
                );
                if let Node::File { identity, .. } = &before {
                    objects.insert(identity.sha256.clone(), identity.size);
                }
                objects.insert(content.sha256.clone(), content.size);
                outputs.push(Output {
                    target: target.clone(),
                    content: Some(content.clone()),
                    mode,
                    mod_id: None,
                });
                journal.changes.push(Change {
                    target,
                    before,
                    after: Node::File {
                        identity: content,
                        mode,
                    },
                });
            }
            journal.validate(&game)?;
            preparing.check()?;
            Ok((PreparedIni { outputs, journal }, objects))
        })
        .await
        .context("Managed INI preparation worker stopped")??;
    control.check()?;
    history.register(&objects).await?;
    Ok(prepared)
}

pub(super) async fn plugins(
    history: &History,
    game: &Game,
    manifest: &Manifest,
    control: Control,
) -> Result<Vec<Output>> {
    ensure!(
        history.game == game.id && manifest.game_id == game.id,
        "Plugin preparation belongs to another game"
    );
    ensure!(
        game.engine == GameEngine::Bethesda && manifest.engine == GameEngine::Bethesda,
        "Plugins.txt preparation belongs to Bethesda"
    );
    control.check()?;
    history.tracker.ensure_location_ready(&game.id).await?;
    let game = game.clone();
    let manifest = manifest.clone();
    let preparing = control.clone();
    let (bytes, slots) = history
        .lease
        .blocking(move || -> Result<_> {
            preparing.check()?;
            let prefix = game
                .wine_prefix
                .as_deref()
                .context("Select the game's Wine prefix before preparing Plugins.txt")?;
            snap::validate_readable_folder(prefix, SelectedFolderKind::WinePrefix)
                .map_err(anyhow::Error::msg)?;
            let _access = std::fs::read_dir(prefix).context(
                "Wine prefix is unavailable; restore prefix access before preparing Plugins.txt",
            )?;
            let slots = game::plugins_txt_paths(&game).len();
            ensure!(
                slots > 0,
                "Plugin configuration locations are unavailable for this game"
            );
            let bytes = render(&manifest)?;
            preparing.check()?;
            Ok((bytes, slots))
        })
        .await
        .context("Plugin preparation worker stopped")??;
    let content = history.retain_generated(bytes, control).await?;
    Ok((0..slots)
        .map(|slot| Output {
            target: Target::PluginControl { slot },
            content: Some(content.clone()),
            mode: 0o644,
            mod_id: None,
        })
        .collect())
}

fn number(row: &Record, key: &str) -> Result<i64> {
    row.get(key)
        .and_then(serde_json::Value::as_i64)
        .with_context(|| format!("Invalid frozen plugin metadata: {key}"))
}

fn render(manifest: &Manifest) -> Result<Vec<u8>> {
    ensure!(
        manifest.engine == GameEngine::Bethesda,
        "Plugins.txt preparation belongs to Bethesda"
    );
    manifest.validate()?;
    let rows = |table| {
        manifest
            .records
            .iter()
            .find(|rows| rows.table == table)
            .map(|rows| rows.rows.as_slice())
            .context("Frozen plugin metadata is incomplete")
    };
    let mut priorities = BTreeMap::new();
    for row in rows(Table::ProfileMods)? {
        priorities.insert(text(row, "mod_id")?, number(row, "priority")?);
    }
    let files = super::prepared::files(manifest)?;
    let mut winners = BTreeMap::new();
    for file in &files {
        if let Target::Bethesda { root: false, path } = &file.target
            && !path.contains('/')
            && file.content.is_some()
        {
            winners.insert(
                path.to_lowercase(),
                file.mod_id
                    .as_deref()
                    .context("Plugin file has no owning mod")?,
            );
        }
    }
    let mut groups: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
    for row in rows(Table::Plugins)? {
        let filename = text(row, "filename")?;
        ensure!(
            relative(filename)?.components().count() == 1
                && filename.trim() == filename
                && !filename.starts_with(['#', '*'])
                && !filename.chars().any(char::is_control),
            "Invalid plugin filename in frozen configuration"
        );
        groups.entry(filename.to_lowercase()).or_default().push(row);
    }
    let mut entries = Vec::new();
    for (filename, candidates) in groups {
        let winner = winners.get(&filename);
        let mut selected = None;
        for row in candidates {
            let mod_id = text(row, "mod_id")?;
            let priority = priorities
                .get(mod_id)
                .context("Plugin owner is missing from the profile")?;
            if winner.is_some_and(|winner| *winner != mod_id) {
                continue;
            }
            let rank = (*priority, std::cmp::Reverse(text(row, "id")?));
            if selected
                .as_ref()
                .is_none_or(|(previous, _)| rank > *previous)
            {
                selected = Some((rank, row));
            }
        }
        let (_, row) =
            selected.context("The deployed plugin file has no matching frozen plugin metadata")?;
        let enabled = number(row, "enabled")?;
        ensure!(matches!(enabled, 0 | 1), "Invalid plugin enabled state");
        entries.push((
            number(row, "load_order")?,
            filename,
            text(row, "filename")?,
            winner.is_some() && enabled == 1,
        ));
    }
    entries.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    let mut bytes = b"# This file is managed by Deployd\n".to_vec();
    for (_, _, filename, enabled) in entries {
        if enabled {
            bytes.push(b'*');
        }
        bytes.extend_from_slice(filename.as_bytes());
        bytes.push(b'\n');
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "../../../tests/generations/bethesda.rs"]
mod tests;
