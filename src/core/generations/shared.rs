use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, Transaction};

use crate::core::game::mass_effect::{
    family::{
        Change, Family,
        generations::{self as engine, Revision},
    },
    journal::State,
};
use crate::models::game::Game;

use super::catalog::History;
use super::content::Control;
use super::journal::{Journal, Node};
use super::target::Target;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub(super) change: Change,
    pub(super) revision: Revision,
    pub(super) games: BTreeSet<String>,
    previous_revision: Option<String>,
    deployed: BTreeMap<String, Option<State>>,
}

impl Intent {
    pub(super) fn validate(&self, game: &Game) -> Result<()> {
        self.revision.validate()?;
        ensure!(
            self.games.contains(&game.id) && self.games.iter().eq(self.deployed.keys()),
            "Shared journal has incomplete affected-game ownership"
        );
        self.change.validate_shared(&game.id, &self.deployed)?;
        ensure!(
            Revision::capture(self.change.location_id, &self.change.desired)? == self.revision,
            "Shared revision differs from its prepared state"
        );
        Ok(())
    }

    pub(super) async fn check(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        game: &Game,
        committed: bool,
        committed_game: Option<&State>,
    ) -> Result<()> {
        self.validate(game)?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mele_journals) OR EXISTS(SELECT 1 FROM generation_journals WHERE game_id LIKE 'mass-effect-le%' AND game_id<>?)")
            .bind(&game.id).fetch_one(&mut **tx).await?;
        ensure!(
            !pending,
            "Another MELE operation requires recovery before shared application"
        );
        let bindings: BTreeSet<String> = sqlx::query_scalar::<_, String>(
            "SELECT game_id FROM game_locations WHERE location_id=? AND role='game'",
        )
        .bind(self.change.location_id)
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .collect();
        ensure!(
            bindings == self.games,
            "Shared launcher bindings changed; recovery information was preserved"
        );
        let document: String =
            sqlx::query_scalar("SELECT document FROM mele_families WHERE location_id=?")
                .bind(self.change.location_id)
                .fetch_one(&mut **tx)
                .await?;
        let family: Family = serde_json::from_str(&document)?;
        ensure!(
            &family
                == if committed {
                    &self.change.desired
                } else {
                    &self.change.previous
                },
            "Shared launcher ownership changed; recovery information was preserved"
        );
        for (id, expected) in &self.deployed {
            let document: Option<String> =
                sqlx::query_scalar("SELECT document FROM mele_deployments WHERE game_id=?")
                    .bind(id)
                    .fetch_optional(&mut **tx)
                    .await?;
            let current: Option<State> = document
                .map(|value| serde_json::from_str(&value))
                .transpose()?;
            let expected = if committed && id == &game.id {
                committed_game.or(expected.as_ref())
            } else {
                expected.as_ref()
            };
            ensure!(
                current.as_ref() == expected,
                "A dependent MELE deployment changed during shared application"
            );
        }
        let current: Option<String> =
            sqlx::query_scalar("SELECT revision_id FROM generation_shared_state WHERE family_id=?")
                .bind(self.change.location_id.to_string())
                .fetch_optional(&mut **tx)
                .await?;
        let expected = if committed {
            Some(self.revision.id()?)
        } else {
            self.previous_revision.clone()
        };
        ensure!(
            current == expected,
            "Shared revision decision changed; recovery information was preserved"
        );
        Ok(())
    }

    pub(super) async fn verify(&self, history: &History, game: &Game, applied: bool) -> Result<()> {
        self.validate(game)?;
        for game in &self.games {
            let bound: String =
                sqlx::query_scalar("SELECT cache_root FROM generation_stores WHERE game_id=?")
                    .bind(game)
                    .fetch_one(&history.tracker.pool)
                    .await?;
            let other = History::bind(
                &history.tracker,
                game,
                std::path::Path::new(&bound),
                false,
                history.lease.clone(),
            )
            .await?;
            let store = other.store.clone();
            let payloads = self.revision.payloads()?;
            history
                .lease
                .blocking(move || -> Result<()> {
                    for identity in payloads.values() {
                        store.verify(identity, &Control::default())?;
                    }
                    Ok(())
                })
                .await
                .context("Shared payload verification stopped")??;
        }
        let tracker = history.tracker.clone();
        let game = game.clone();
        let change = self.change.clone();
        history
            .lease
            .participant(async move { change.verify_generation(&tracker, &game, applied).await })
            .await
            .context("Shared launcher verification stopped")?
    }

    pub(super) async fn protect(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        operation: &str,
    ) -> Result<()> {
        for game in &self.games {
            for identity in self.revision.payloads()?.values() {
                sqlx::query("INSERT OR IGNORE INTO generation_pending_objects(operation_id,game_id,sha256) VALUES (?,?,?)").bind(operation).bind(game).bind(&identity.sha256).execute(&mut **tx).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn publish(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        game: &Game,
    ) -> Result<()> {
        self.check(tx, game, false, None).await?;
        publish(tx, &self.revision, &self.games).await?;
        sqlx::query("UPDATE mele_families SET document=? WHERE location_id=?")
            .bind(serde_json::to_string(&self.change.desired)?)
            .bind(self.change.location_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("INSERT INTO generation_shared_state(family_id,revision_id) VALUES (?,?) ON CONFLICT(family_id) DO UPDATE SET revision_id=excluded.revision_id").bind(self.change.location_id.to_string()).bind(self.revision.id()?).execute(&mut **tx).await?;
        Ok(())
    }
}

pub(super) async fn publish(
    tx: &mut Transaction<'_, Sqlite>,
    revision: &Revision,
    games: &BTreeSet<String>,
) -> Result<()> {
    let family = revision.location.to_string();
    let id = revision.id()?;
    let document = serde_json::to_string(revision)?;
    let previous: Option<String> = sqlx::query_scalar(
        "SELECT manifest FROM generation_shared_revisions WHERE family_id=? AND id=?",
    )
    .bind(&family)
    .bind(&id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(previous) = previous {
        ensure!(
            previous == document,
            "Immutable shared revision identity collision"
        );
    } else {
        sqlx::query("INSERT INTO generation_shared_revisions(family_id,id,created_at,manifest_version,manifest) VALUES (?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now'),1,?)").bind(&family).bind(&id).bind(document).execute(&mut **tx).await?;
    }
    for game in games {
        for identity in revision.payloads()?.values() {
            let size: Option<i64> = sqlx::query_scalar(
                "SELECT size FROM generation_objects WHERE game_id=? AND sha256=?",
            )
            .bind(game)
            .bind(&identity.sha256)
            .fetch_optional(&mut **tx)
            .await?;
            ensure!(
                size == Some(i64::try_from(identity.size)?),
                "Shared payload is missing from an affected game's store"
            );
            sqlx::query("INSERT OR IGNORE INTO generation_shared_objects(family_id,revision_id,game_id,sha256) VALUES (?,?,?,?)").bind(&family).bind(&id).bind(game).bind(&identity.sha256).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

pub(super) async fn prepare(
    history: &History,
    game: &Game,
    recipe: crate::core::game::mass_effect::recipe::Recipe,
    data: PathBuf,
    control: Control,
) -> Result<Journal> {
    idle(history).await?;
    let tracker = history.tracker.clone();
    let preparing_game = game.clone();
    let preparing = control.clone();
    let prepared = history
        .lease
        .participant(async move {
            engine::prepare(tracker, preparing_game, recipe, data, preparing).await
        })
        .await
        .context("Shared launcher preparation stopped")??;
    let result = async {
        for id in &prepared.games {
            let other = binding(history, id).await?;
            for (path, expected) in prepared.revision.payloads()? {
                ensure!(
                    other
                        .retain(prepared.payload(&path)?, control.clone())
                        .await?
                        == expected,
                    "Shared historical payload changed during retention"
                );
            }
        }
        let mut desired = Vec::new();
        for (path, (before, after)) in prepared.change.generation_files() {
            if before.is_none() && after.is_none() {
                continue;
            }
            let node = if let Some(identity) = after {
                ensure!(
                    history
                        .retain(prepared.source(&path), control.clone())
                        .await?
                        == identity,
                    "Prepared launcher output changed"
                );
                Node::File {
                    identity,
                    mode: 0o644,
                }
            } else {
                Node::Absent
            };
            desired.push((Target::MeleLauncher { path }, node));
        }
        prepared.verify_sources().await?;
        let mut deployed = BTreeMap::new();
        for id in &prepared.games {
            deployed.insert(id.clone(), history.tracker.mele_deployment(id).await?);
        }
        let previous_revision =
            sqlx::query_scalar("SELECT revision_id FROM generation_shared_state WHERE family_id=?")
                .bind(prepared.change.location_id.to_string())
                .fetch_optional(&history.tracker.pool)
                .await?;
        let intent = Intent {
            change: prepared.change.clone(),
            revision: prepared.revision.clone(),
            games: prepared.games.clone(),
            previous_revision,
            deployed,
        };
        let mut journal = Journal::prepare(history, game, desired, control.clone()).await?;
        journal.attach_shared(game, intent)?;
        Ok(journal)
    }
    .await;
    prepared.discard().await?;
    result
}

async fn idle(history: &History) -> Result<()> {
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id LIKE 'mass-effect-le%') OR EXISTS(SELECT 1 FROM mele_journals)")
        .fetch_one(&history.tracker.pool).await?;
    ensure!(
        !pending,
        "Finish MELE recovery before preparing shared changes"
    );
    Ok(())
}

async fn binding(history: &History, game: &str) -> Result<History> {
    let bound: Option<String> =
        sqlx::query_scalar("SELECT cache_root FROM generation_stores WHERE game_id=?")
            .bind(game)
            .fetch_optional(&history.tracker.pool)
            .await?;
    let create = bound.is_none();
    let cache = if let Some(bound) = bound {
        PathBuf::from(bound)
    } else {
        let custom: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key=?")
            .bind(format!("cache_dir_{game}"))
            .fetch_optional(&history.tracker.pool)
            .await?;
        crate::utils::paths::game_cache_root(custom.as_deref().map(std::path::Path::new))?
    };
    History::bind(
        &history.tracker,
        game,
        &cache,
        create,
        history.lease.clone(),
    )
    .await
}

pub(super) fn identity(revision: Option<&Revision>) -> Result<Option<(String, String)>> {
    revision
        .map(|revision| Ok((revision.location.to_string(), revision.id()?)))
        .transpose()
}

pub(super) async fn verify_dependency(
    history: &History,
    game: &Game,
    desired: &State,
    expected: Option<&Revision>,
) -> Result<()> {
    let tracker = history.tracker.clone();
    let game = game.clone();
    let desired = desired.clone();
    let current = history
        .lease
        .participant(async move { engine::dependency(&tracker, &game, &desired).await })
        .await
        .context("Shared dependency verification stopped")??;
    ensure!(
        current.as_ref() == expected,
        "The live shared launcher differs from this generation; prepare the game deployment again to include its parent-owned launcher components"
    );
    Ok(())
}

pub(super) async fn commit_dependency(
    tx: &mut Transaction<'_, Sqlite>,
    game: &str,
    revision: &Revision,
) -> Result<()> {
    let document: String =
        sqlx::query_scalar("SELECT document FROM mele_families WHERE location_id=?")
            .bind(revision.location)
            .fetch_one(&mut **tx)
            .await?;
    let family: Family = serde_json::from_str(&document)?;
    ensure!(
        Revision::capture(revision.location, &family)? == *revision,
        "Shared launcher dependency changed before commitment"
    );
    publish(tx, revision, &BTreeSet::from([game.to_owned()])).await?;
    sqlx::query("INSERT INTO generation_shared_state(family_id,revision_id) VALUES (?,?) ON CONFLICT(family_id) DO UPDATE SET revision_id=excluded.revision_id")
        .bind(revision.location.to_string()).bind(revision.id()?).execute(&mut **tx).await?;
    Ok(())
}

pub(super) async fn load(
    history: &History,
    family: &str,
    id: &str,
    control: Control,
) -> Result<Revision> {
    let (version, document): (i64, String) = sqlx::query_as("SELECT manifest_version,manifest FROM generation_shared_revisions WHERE family_id=? AND id=?")
        .bind(family).bind(id).fetch_one(&history.tracker.pool).await.context("Shared revision is unavailable")?;
    ensure!(
        version == 1,
        "Shared revision requires a compatible Deployd version"
    );
    let revision: Revision = serde_json::from_str(&document)?;
    ensure!(
        revision.location.to_string() == family && revision.id()? == id,
        "Shared revision is damaged"
    );
    let objects: BTreeMap<String, i64> = sqlx::query_as::<_, (String, i64)>("SELECT o.sha256,o.size FROM generation_shared_objects r JOIN generation_objects o ON o.game_id=r.game_id AND o.sha256=r.sha256 WHERE r.family_id=? AND r.revision_id=? AND r.game_id=?")
        .bind(family).bind(id).bind(&history.game).fetch_all(&history.tracker.pool).await?.into_iter().collect();
    let expected = revision
        .payloads()?
        .into_values()
        .map(|id| Ok((id.sha256, i64::try_from(id.size)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    ensure!(
        objects == expected,
        "Shared payload references are incomplete for this game"
    );
    let store = history.store.clone();
    let checking = revision.clone();
    history
        .lease
        .blocking(move || -> Result<()> {
            for id in checking.payloads()?.values() {
                store.verify(id, &control)?;
            }
            Ok(())
        })
        .await
        .context("Shared payload verification stopped")??;
    Ok(revision)
}
