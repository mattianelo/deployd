use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite, Transaction};

use crate::core::save_manager::SaveSetId;
use crate::models::game::Game;
use crate::models::manifest::ModFile;

use super::catalog::History;
use super::manifest::Manifest;
use super::target::Target;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub(super) generation: Option<String>,
    pub(super) profile: Option<String>,
    pub(super) saves: SaveSetId,
    pub(super) modified: bool,
}

pub(super) struct Deployment<'a> {
    pub(super) manifest: &'a Manifest,
    pub(super) profile: &'a str,
    pub(super) files: &'a [ModFile],
}

pub(super) async fn read(tx: &mut Transaction<'_, Sqlite>, game: &str) -> Result<Option<State>> {
    let row = sqlx::query("SELECT deployed_generation_id,deployed_profile_id,live_save_profile_id,live_save_mode,modified FROM generation_game_state WHERE game_id=?")
        .bind(game).fetch_optional(&mut **tx).await?;
    row.map(|row| {
        let mode: String = row.try_get("live_save_mode")?;
        let profile: Option<String> = row.try_get("live_save_profile_id")?;
        let saves = match (mode.as_str(), profile) {
            ("global", None) => SaveSetId::Global {
                game_id: game.to_owned(),
            },
            ("profile", Some(profile_id)) => SaveSetId::Profile {
                game_id: game.to_owned(),
                profile_id,
            },
            _ => anyhow::bail!(
                "Live save ownership is inconsistent; dependent operations are blocked"
            ),
        };
        Ok(State {
            generation: row.try_get("deployed_generation_id")?,
            profile: row.try_get("deployed_profile_id")?,
            saves,
            modified: row.try_get("modified")?,
        })
    })
    .transpose()
}

pub(super) async fn publish(
    tx: &mut Transaction<'_, Sqlite>,
    history: &History,
    game: &Game,
    activation: &str,
    deployment: Option<&Deployment<'_>>,
    saves: &SaveSetId,
) -> Result<()> {
    ensure!(
        game.id == history.game && saves.game_id() == game.id,
        "Activation and save ownership belong to different games"
    );
    let (generation, profile, kind) = if let Some(deployment) = deployment {
        ensure!(
            deployment.manifest.engine == game.engine,
            "Generation belongs to another engine"
        );
        let mode: String =
            sqlx::query_scalar("SELECT save_mode FROM profiles WHERE game_id=? AND id=?")
                .bind(&game.id)
                .bind(deployment.profile)
                .fetch_one(&mut **tx)
                .await
                .context("The activation profile no longer exists")?;
        ensure!(
            (mode == "profile") == saves.profile_id().is_some(),
            "The profile save mode changed after activation preparation"
        );
        ensure!(
            saves
                .profile_id()
                .is_none_or(|profile| profile == deployment.profile),
            "Activation cannot assign another profile's save bank"
        );
        validate_files(history, game, deployment)?;
        let generation = history.publish(tx, deployment.manifest).await?;
        (Some(generation), Some(deployment.profile), "deploy")
    } else {
        (None, None, "purge")
    };
    sqlx::query("DELETE FROM deployed_files WHERE game_id=?")
        .bind(&game.id)
        .execute(&mut **tx)
        .await?;
    if let Some(deployment) = deployment {
        for file in deployment.files {
            sqlx::query("INSERT INTO deployed_files(game_id,game_rel_lowercase,game_rel_original,mod_id,cache_path) VALUES (?,?,?,?,?)")
                .bind(&game.id).bind(&file.game_rel_lowercase).bind(&file.game_rel_original).bind(&file.mod_id).bind(&file.cache_path).execute(&mut **tx).await?;
        }
    }
    sqlx::query("INSERT INTO generation_game_state(game_id,deployed_generation_id,deployed_profile_id,live_save_profile_id,live_save_mode,modified) VALUES (?,?,?,?,?,0) ON CONFLICT(game_id) DO UPDATE SET deployed_generation_id=excluded.deployed_generation_id,deployed_profile_id=excluded.deployed_profile_id,live_save_profile_id=excluded.live_save_profile_id,live_save_mode=excluded.live_save_mode,modified=0")
        .bind(&game.id).bind(&generation).bind(profile).bind(saves.profile_id()).bind(if saves.profile_id().is_some() { "profile" } else { "global" }).execute(&mut **tx).await?;
    let key = format!("last_deployed_profile_{}", game.id);
    if let Some(profile) = profile {
        sqlx::query("INSERT INTO settings(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value")
            .bind(&key).bind(profile).execute(&mut **tx).await?;
        sqlx::query(
            "UPDATE generation_drafts SET seed_live_saves=0 WHERE game_id=? AND profile_id=?",
        )
        .bind(&game.id)
        .bind(profile)
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query("DELETE FROM settings WHERE key=?")
            .bind(&key)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("INSERT INTO generation_activations(id,game_id,generation_id,profile_id,created_at,kind) VALUES (?,?,?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now'),?)")
        .bind(activation).bind(&game.id).bind(generation).bind(profile).bind(kind).execute(&mut **tx).await?;
    Ok(())
}

fn validate_files(history: &History, game: &Game, deployment: &Deployment<'_>) -> Result<()> {
    if game.engine != crate::models::game::GameEngine::MassEffect {
        for output in super::prepared::files(deployment.manifest)? {
            ensure!(
                deployment.manifest.outputs.contains(&output),
                "The generation does not include its complete prepared mod configuration"
            );
        }
    }
    let mut expected = std::collections::BTreeMap::new();
    for output in deployment
        .manifest
        .outputs
        .iter()
        .filter(|output| output.mod_id.is_some())
    {
        ensure!(
            expected
                .insert(
                    output
                        .target
                        .resolve(game)?
                        .to_string_lossy()
                        .to_lowercase(),
                    output
                )
                .is_none(),
            "Historical output destinations overlap"
        );
    }
    let mut actual = std::collections::BTreeSet::new();
    for file in deployment.files {
        ensure!(
            file.game_rel_original.to_lowercase() == file.game_rel_lowercase,
            "Deployment record has inconsistent routing"
        );
        let target = Target::file(&game.engine, &file.game_rel_original)?;
        let destination = target.resolve(game)?.to_string_lossy().to_lowercase();
        ensure!(
            actual.insert(destination.clone()),
            "Deployment records contain duplicate destinations"
        );
        let output = expected
            .get(&destination)
            .context("Deployment record is absent from the prepared generation")?;
        ensure!(
            output.mod_id.as_deref() == Some(file.mod_id.as_str()),
            "Deployment record assigns content to a different mod"
        );
        ensure!(
            output.content.is_none() == file.game_rel_original.ends_with('/'),
            "Deployment record has the wrong file type"
        );
        let suffix = std::path::Path::new(&file.cache_path)
            .strip_prefix(&history.cache)
            .context("Deployment cache reference is outside the authorized cache")?;
        super::target::relative(
            suffix
                .to_str()
                .context("Deployment cache path is not UTF-8")?,
        )?;
    }
    ensure!(
        actual.len() == expected.len(),
        "Deployment records do not cover all prepared managed outputs"
    );
    Ok(())
}
