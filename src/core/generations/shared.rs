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

use super::catalog::{History, durable};
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
            ensure!(
                &current == expected,
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
        self.check(tx, game, false).await?;
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
    action: engine::Action,
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
            engine::prepare(tracker, preparing_game, action, data, preparing).await
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

pub(super) async fn apply(
    history: &History,
    game: &Game,
    journal: &Journal,
    control: Control,
) -> Result<()> {
    ensure!(
        journal.shared.is_some(),
        "Shared Apply requires its prepared participant"
    );
    journal
        .verify_prepared(history, game, Vec::new(), control.clone())
        .await?;
    journal
        .persist(history, game, "shared", BTreeMap::new())
        .await?;
    let attempt = async {
        let applied = journal.apply(history, game, control.clone()).await?;
        control.check()?;
        applied.commit_shared(history, game).await
    }
    .await;
    super::coordinator::finish(history, game, journal, attempt).await
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

pub(super) async fn capture_dependency(
    history: &History,
    game: &Game,
    desired: &State,
    data: &std::path::Path,
    control: Control,
) -> Result<Option<Revision>> {
    let tracker = history.tracker.clone();
    let verifying_game = game.clone();
    let desired = desired.clone();
    let revision = history
        .lease
        .participant(async move { engine::dependency(&tracker, &verifying_game, &desired).await })
        .await
        .context("Shared dependency inspection stopped")??;
    if let Some(revision) = &revision {
        let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_shared_objects WHERE family_id=? AND revision_id=? AND game_id=?)")
            .bind(revision.location.to_string()).bind(revision.id()?).bind(&history.game).fetch_one(&history.tracker.pool).await?;
        if retained {
            load(
                history,
                &revision.location.to_string(),
                &revision.id()?,
                control,
            )
            .await?;
            return Ok(Some(revision.clone()));
        }
        for (path, expected) in revision.payloads()? {
            ensure!(
                history
                    .retain(revision.source(data, &path)?, control.clone())
                    .await?
                    == expected,
                "Shared dependency payload changed during retention"
            );
        }
    }
    Ok(revision)
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
        "The live shared launcher differs from this generation; explicitly Apply or Restore its shared revision before Deploy"
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

pub(super) async fn restore(
    history: &History,
    game: &Game,
    family: &str,
    id: &str,
    control: Control,
) -> Result<Journal> {
    idle(history).await?;
    let desired = load(history, family, id, control.clone()).await?;
    let current = history
        .tracker
        .mele_family(desired.location)
        .await?
        .context("Shared launcher state is unavailable")?;
    desired.restore(&current)?;
    let current = Revision::capture(desired.location, &current)?;
    let current = load(history, family, &current.id()?, control.clone()).await?;
    let store = history.store.clone();
    let cache = history.cache.clone();
    let copying = control.clone();
    let restoring = desired.clone();
    let directory = history
        .lease
        .blocking(move || -> Result<_> {
            let directory = tempfile::Builder::new()
                .prefix(".shared-history-")
                .tempdir_in(cache)?;
            let mut paths = BTreeMap::new();
            for revision in [current, restoring] {
                for (logical, identity) in revision.payloads()? {
                    let path = revision.source(directory.path(), &logical)?;
                    if let Some(previous) = paths.insert(path.clone(), identity.clone()) {
                        ensure!(
                            previous == identity,
                            "Shared revisions disagree on required retained inputs"
                        );
                        continue;
                    }
                    std::fs::create_dir_all(
                        path.parent().context("Shared payload has no parent")?,
                    )?;
                    store.materialize(&identity, &path, 0o600, &copying)?;
                }
            }
            Ok(directory)
        })
        .await
        .context("Shared restoration materialization stopped")??;
    let result = prepare(
        history,
        game,
        engine::Action::Restore(desired),
        directory.path().into(),
        control,
    )
    .await;
    history
        .lease
        .blocking(move || drop(directory))
        .await
        .context("Shared restoration cleanup stopped")?;
    result
}

#[derive(Debug)]
pub(super) struct Entry {
    pub(super) id: String,
    pub(super) created_at: String,
    pub(super) live: bool,
    pub(super) references: i64,
}

pub(super) async fn list(history: &History, family: &str) -> Result<Vec<Entry>> {
    use sqlx::Row;
    sqlx::query("SELECT r.id,r.created_at,EXISTS(SELECT 1 FROM generation_shared_state s WHERE s.family_id=r.family_id AND s.revision_id=r.id) AS live,(SELECT COUNT(*) FROM generation_shared_dependencies d WHERE d.family_id=r.family_id AND d.revision_id=r.id) AS refs FROM generation_shared_revisions r WHERE r.family_id=? AND EXISTS(SELECT 1 FROM generation_shared_objects o WHERE o.family_id=r.family_id AND o.revision_id=r.id AND o.game_id=?) ORDER BY r.created_at DESC,r.id")
        .bind(family).bind(&history.game).fetch_all(&history.tracker.pool).await?.into_iter().map(|row| Ok(Entry {
            id: row.try_get("id")?, created_at: row.try_get("created_at")?, live: row.try_get("live")?, references: row.try_get("refs")?,
        })).collect()
}

async fn deletable(tx: &mut Transaction<'_, Sqlite>, family: &str, id: &str) -> Result<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM generation_shared_revisions WHERE family_id=? AND id=?)",
    )
    .bind(family)
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    ensure!(exists, "Shared revision is unavailable");
    let protected: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_shared_state WHERE family_id=? AND revision_id=?) OR EXISTS(SELECT 1 FROM generation_shared_dependencies WHERE family_id=? AND revision_id=?) OR EXISTS(SELECT 1 FROM generation_journals WHERE game_id LIKE 'mass-effect-le%')")
        .bind(family).bind(id).bind(family).bind(id).fetch_one(&mut **tx).await?;
    ensure!(
        !protected,
        "Shared revision is live or referenced by game history or pending recovery"
    );
    Ok(())
}

async fn reclaimable(
    tx: &mut Transaction<'_, Sqlite>,
    family: &str,
    id: &str,
) -> Result<Vec<(String, String, i64)>> {
    Ok(sqlx::query_as("SELECT o.game_id,o.sha256,o.size FROM generation_shared_objects r JOIN generation_objects o ON o.game_id=r.game_id AND o.sha256=r.sha256 WHERE r.family_id=? AND r.revision_id=? AND NOT EXISTS(SELECT 1 FROM generation_object_references g WHERE g.game_id=o.game_id AND g.sha256=o.sha256) AND NOT EXISTS(SELECT 1 FROM generation_pending_objects p WHERE p.game_id=o.game_id AND p.sha256=o.sha256) AND NOT EXISTS(SELECT 1 FROM generation_shared_objects other WHERE other.game_id=o.game_id AND other.sha256=o.sha256 AND (other.family_id<>r.family_id OR other.revision_id<>r.revision_id))")
        .bind(family).bind(id).fetch_all(&mut **tx).await?)
}

pub(super) async fn deletion_size(history: &History, family: &str, id: &str) -> Result<u64> {
    let mut tx = history.tracker.pool.begin().await?;
    deletable(&mut tx, family, id).await?;
    reclaimable(&mut tx, family, id)
        .await?
        .into_iter()
        .try_fold(0u64, |total, (_, _, size)| {
            total
                .checked_add(size.try_into()?)
                .context("Shared history usage exceeds the supported range")
        })
}

pub(super) async fn delete(history: &History, family: &str, id: &str) -> Result<()> {
    let mut tx = durable(&history.tracker).await?;
    deletable(&mut tx, family, id).await?;
    let objects = reclaimable(&mut tx, family, id).await?;
    sqlx::query("DELETE FROM generation_shared_revisions WHERE family_id=? AND id=?")
        .bind(family)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let mut games = BTreeSet::new();
    for (game, hash, _) in objects {
        sqlx::query("INSERT INTO generation_deletions(game_id,sha256) VALUES (?,?)")
            .bind(&game)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        games.insert(game);
    }
    tx.commit().await?;
    for game in games {
        let bound: String =
            sqlx::query_scalar("SELECT cache_root FROM generation_stores WHERE game_id=?")
                .bind(&game)
                .fetch_one(&history.tracker.pool)
                .await?;
        let other = History::bind(
            &history.tracker,
            &game,
            std::path::Path::new(&bound),
            false,
            history.lease.clone(),
        )
        .await
        .context(
            "Shared deletion was recorded; reconnect affected caches to finish payload cleanup",
        )?;
        other.finish_deletions().await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/generations/shared.rs"]
mod tests;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) async fn dependency_matches(
    history: &History,
    game: &Game,
    manifest: &super::manifest::Manifest,
) -> Result<bool> {
    ensure!(
        history.game == game.id
            && manifest.game_id == game.id
            && game.engine == crate::models::game::GameEngine::MassEffect,
        "Shared dependency status belongs to another game"
    );
    let location = history
        .tracker
        .folder_location(&game.id, crate::utils::location::FolderRole::Game)
        .await?;
    let revision = history
        .tracker
        .mele_family(location.id)
        .await?
        .map(|family| Revision::capture(location.id, &family))
        .transpose()?;
    Ok(identity(revision.as_ref())? == manifest.shared_revision)
}
