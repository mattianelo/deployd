use std::fs;
use std::path::Component;

use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, Transaction};

use crate::models::game::Game;
use crate::utils::snap::{self, SelectedFolderKind};

use super::catalog::{History, durable};
use super::content::Control;
use super::journal::{self, Node};
use super::manifest::Output;
use super::state::{self, State};
use super::target::Target;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Difference {
    pub(super) target: Target,
    pub(super) expected: Node,
    pub(super) actual: Node,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Inspection {
    pub(super) generation: String,
    pub(super) differences: Vec<Difference>,
}

pub(super) async fn inspect(
    history: &History,
    game: &Game,
    control: Control,
) -> Result<Option<Inspection>> {
    ensure!(
        history.game == game.id,
        "Deployment inspection belongs to another game"
    );
    control.check()?;
    history.tracker.ensure_location_ready(&game.id).await?;
    let mut tx = durable(&history.tracker).await?;
    let previous = ready(&mut tx, &game.id).await?;
    tx.rollback().await?;
    let Some(generation) = previous.as_ref().and_then(|state| state.generation.clone()) else {
        return Ok(None);
    };
    let manifest = history
        .load_with_control(&generation, control.clone())
        .await?;
    ensure!(
        manifest.engine == game.engine,
        "Deployment inspection belongs to another engine"
    );
    let game_copy = game.clone();
    let scanning = control.clone();
    let differences = history
        .lease
        .blocking(move || scan(&game_copy, &manifest.outputs, &scanning))
        .await
        .context("Deployment inspection worker stopped")??;
    control.check()?;
    let mut tx = durable(&history.tracker).await?;
    ensure!(
        ready(&mut tx, &game.id).await? == previous,
        "Deployed state changed during inspection; inspect the current deployment again"
    );
    sqlx::query("UPDATE generation_game_state SET modified=? WHERE game_id=?")
        .bind(!differences.is_empty())
        .bind(&game.id)
        .execute(&mut *tx)
        .await?;
    control.check()?;
    tx.commit()
        .await
        .context("Cannot persist deployment modification status")?;
    Ok(Some(Inspection {
        generation,
        differences,
    }))
}

async fn ready(tx: &mut Transaction<'_, Sqlite>, game: &str) -> Result<Option<State>> {
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
            .bind(game)
            .fetch_one(&mut **tx)
            .await?;
    ensure!(
        !pending,
        "Finish pending generation recovery before inspecting live deployment"
    );
    state::read(tx, game).await
}

fn scan(game: &Game, outputs: &[Output], control: &Control) -> Result<Vec<Difference>> {
    if outputs.is_empty() {
        snap::validate_readable_folder(&game.path, SelectedFolderKind::GameFolder)
            .map_err(anyhow::Error::msg)?;
        let _access = fs::read_dir(&game.path).context(
            "Deployment location is unavailable; restore folder access before inspecting",
        )?;
    }
    let mut differences = Vec::new();
    for output in outputs {
        control.check()?;
        let actual = live(game, &output.target, control)?;
        let expected = match &output.content {
            Some(identity) => Node::File {
                identity: identity.clone(),
                mode: output.mode,
            },
            None => Node::Directory { mode: output.mode },
        };
        if actual != expected {
            differences.push(Difference {
                target: output.target.clone(),
                expected,
                actual,
            });
        }
    }
    Ok(differences)
}

pub(super) fn live(game: &Game, target: &Target, control: &Control) -> Result<Node> {
    target.validate(&game.engine)?;
    let (root, kind) = match target {
        Target::Eclipse { .. } | Target::PluginControl { .. } | Target::CustomIni { .. } => (
            game.wine_prefix.as_deref().context(
                "Wine prefix is unavailable; restore access before inspecting deployment",
            )?,
            SelectedFolderKind::WinePrefix,
        ),
        _ => (game.path.as_path(), SelectedFolderKind::GameFolder),
    };
    snap::validate_readable_folder(root, kind).map_err(anyhow::Error::msg)?;
    let _access = fs::read_dir(root)
        .context("Deployment location is unavailable; restore folder access before inspecting")?;
    let path = target.resolve(game)?;
    ensure!(
        path != root,
        "Managed target resolves to its authorized root"
    );
    let relative = path
        .strip_prefix(root)
        .context("Managed target escapes its authorized location")?;
    ensure!(
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "Managed target escapes its authorized location"
    );
    let mut parent = root.to_owned();
    if let Some(parents) = relative.parent() {
        for component in parents.components() {
            control.check()?;
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) => ensure!(
                    metadata.is_dir(),
                    "Managed parent is a symbolic link or non-directory; inspection stopped: {}",
                    parent.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Node::Absent);
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Cannot inspect managed parent '{}'", parent.display())
                    });
                }
            }
        }
    }
    journal::inspect(&path, control)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../../../tests/generations/divergence.rs"]
mod integration_tests;
