use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, Transaction};

use crate::core::tracker::Tracker;

use super::content::{Control, Identity};
use super::operation::Lease;
use super::store::Store;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Initialization {
    initialize_store: String,
}

pub(super) struct History {
    pub(super) tracker: Tracker,
    pub(super) game: String,
    pub(super) cache: PathBuf,
    pub(super) store: Arc<Store>,
    pub(super) lease: Arc<Lease>,
}

pub(super) async fn durable(tracker: &Tracker) -> Result<Transaction<'_, Sqlite>> {
    let mut tx = tracker.pool.begin().await?;
    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut *tx)
        .await?;
    ensure!(
        synchronous >= 2,
        "Deployment history requires fully durable database writes; restart Deployd with its default database settings"
    );
    Ok(tx)
}

impl History {
    pub(super) async fn open(
        tracker: &Tracker,
        game: &str,
        cache: &Path,
        create: bool,
    ) -> Result<Self> {
        Self::bind(tracker, game, cache, create, Lease::acquire().await?).await
    }

    pub(super) async fn bind(
        tracker: &Tracker,
        game: &str,
        cache: &Path,
        create: bool,
        lease: Arc<Lease>,
    ) -> Result<Self> {
        crate::utils::paths::generation_store_in(cache, game)?;
        let mut tx = durable(tracker).await?;
        let pending_version: Option<(i64, String)> =
            sqlx::query_as("SELECT document_version,kind FROM generation_journals WHERE game_id=?")
                .bind(game)
                .fetch_optional(&mut *tx)
                .await?;
        ensure!(
            pending_version.is_none_or(|(version, kind)| version == 1
                || (matches!(version, 2..=5)
                    && matches!(kind.as_str(), "deploy" | "purge" | "shared"))),
            "A newer recovery journal requires a compatible Deployd version; the pending operation was preserved"
        );
        let binding: Option<(String, String)> =
            sqlx::query_as("SELECT store_id,cache_root FROM generation_stores WHERE game_id=?")
                .bind(game)
                .fetch_optional(&mut *tx)
                .await?;
        let (id, initializing) = if let Some((id, root)) = binding {
            ensure!(
                Path::new(&root) == cache,
                "History is bound to another cache location; finish cache relocation or restore access before continuing"
            );
            let pending: Option<(String, i64, String)> = sqlx::query_as("SELECT id,document_version,document FROM generation_journals WHERE game_id=? AND kind='relocate' AND committed=0").bind(game).fetch_optional(&mut *tx).await?;
            let initialize = pending.and_then(|(journal, version, document)| {
                serde_json::from_str::<Initialization>(&document)
                    .ok()
                    .filter(|value| version == 1 && value.initialize_store == id)
                    .map(|_| journal)
            });
            (id, initialize)
        } else {
            ensure!(
                create,
                "No retained deployment history exists for this game"
            );
            let id = uuid::Uuid::new_v4().to_string();
            let journal = uuid::Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO generation_stores(game_id,store_id,cache_root) VALUES (?,?,?)",
            )
            .bind(game)
            .bind(&id)
            .bind(cache.to_str().context("Cache path is not UTF-8")?)
            .execute(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,'relocate',1,?)").bind(&journal).bind(game).bind(serde_json::to_string(&Initialization { initialize_store: id.clone() })?).execute(&mut *tx).await?;
            (id, Some(journal))
        };
        tx.commit().await?;
        let root = cache.to_owned();
        let game_id = game.to_owned();
        let initialize = initializing.is_some();
        let store = lease
            .blocking(move || {
                if initialize {
                    Store::create(&root, &game_id, &id)
                } else {
                    Store::open(&root, &game_id, &id)
                }
            })
            .await
            .context("History storage worker stopped")??;
        if let Some(journal) = initializing {
            let mut tx = durable(tracker).await?;
            sqlx::query("DELETE FROM generation_journals WHERE id=? AND committed=0")
                .bind(journal)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(Self {
            tracker: tracker.clone(),
            game: game.to_owned(),
            cache: cache.to_owned(),
            store: Arc::new(store),
            lease,
        })
    }

    pub(super) async fn register(
        &self,
        objects: &std::collections::BTreeMap<String, u64>,
    ) -> Result<()> {
        let mut tx = durable(&self.tracker).await?;
        for (hash, size) in objects {
            let previous: Option<i64> = sqlx::query_scalar(
                "SELECT size FROM generation_objects WHERE game_id=? AND sha256=?",
            )
            .bind(&self.game)
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(previous) = previous {
                ensure!(
                    previous as u64 == *size,
                    "Inconsistent retained content metadata"
                );
            } else {
                sqlx::query("INSERT INTO generation_objects(game_id,sha256,size) VALUES (?,?,?)")
                    .bind(&self.game)
                    .bind(hash)
                    .bind(i64::try_from(*size)?)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn retain(&self, source: PathBuf, control: Control) -> Result<Identity> {
        let store = self.store.clone();
        let identity = self
            .lease
            .blocking(move || store.retain(&source, &control))
            .await
            .context("Retained copy worker stopped")??;
        let mut tx = durable(&self.tracker).await?;
        let previous: Option<i64> =
            sqlx::query_scalar("SELECT size FROM generation_objects WHERE game_id=? AND sha256=?")
                .bind(&self.game)
                .bind(&identity.sha256)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(size) = previous {
            ensure!(
                size as u64 == identity.size,
                "Retained content identity has inconsistent metadata"
            );
        } else {
            sqlx::query("INSERT INTO generation_objects(game_id,sha256,size) VALUES (?,?,?)")
                .bind(&self.game)
                .bind(&identity.sha256)
                .bind(identity.size as i64)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(identity)
    }

    pub(super) async fn retain_generated(
        &self,
        bytes: Vec<u8>,
        control: Control,
    ) -> Result<Identity> {
        let store = self.store.clone();
        let identity = self
            .lease
            .blocking(move || store.retain_generated(&bytes, &control))
            .await
            .context("Generated content retention worker stopped")??;
        self.register(&std::collections::BTreeMap::from([(
            identity.sha256.clone(),
            identity.size,
        )]))
        .await?;
        Ok(identity)
    }

    pub(super) async fn publish(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        manifest: &super::manifest::Manifest,
    ) -> Result<String> {
        ensure!(
            manifest.game_id == self.game,
            "Generation belongs to another game"
        );
        let id = manifest.id()?;
        for (hash, size) in manifest.objects() {
            let registered: Option<i64> = sqlx::query_scalar(
                "SELECT size FROM generation_objects WHERE game_id=? AND sha256=?",
            )
            .bind(&self.game)
            .bind(hash)
            .fetch_optional(&mut **tx)
            .await?;
            ensure!(
                registered == Some(i64::try_from(size)?),
                "Generation content was not completely retained; activation cannot commit"
            );
        }
        let document = serde_json::to_string(manifest)?;
        let previous: Option<String> =
            sqlx::query_scalar("SELECT manifest FROM generations WHERE game_id=? AND id=?")
                .bind(&self.game)
                .bind(&id)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(previous) = previous {
            ensure!(
                previous == document,
                "Immutable generation identity collision"
            );
            return Ok(id);
        }
        let (profile, name) = manifest.profile()?;
        sqlx::query("INSERT INTO generations(game_id,id,created_at,originating_profile_id,originating_profile_name,manifest_version,manifest) VALUES (?,?,strftime('%Y-%m-%dT%H:%M:%fZ','now'),?,?,?,?)").bind(&self.game).bind(&id).bind(profile).bind(name).bind(manifest.version).bind(document).execute(&mut **tx).await?;
        if let Some((family, revision)) = &manifest.shared_revision {
            sqlx::query("INSERT INTO generation_shared_dependencies(game_id,generation_id,family_id,revision_id) VALUES (?,?,?,?)")
                .bind(&self.game).bind(&id).bind(family).bind(revision).execute(&mut **tx).await?;
        }
        for (hash, _) in manifest.objects() {
            sqlx::query("INSERT INTO generation_object_references(game_id,generation_id,sha256) VALUES (?,?,?)").bind(&self.game).bind(&id).bind(hash).execute(&mut **tx).await?;
        }
        Ok(id)
    }

    pub(super) async fn load(&self, id: &str) -> Result<super::manifest::Manifest> {
        self.load_with_control(id, Control::default()).await
    }

    pub(super) async fn load_with_control(
        &self,
        id: &str,
        control: Control,
    ) -> Result<super::manifest::Manifest> {
        control.check()?;
        let (version, document): (i64, String) = sqlx::query_as(
            "SELECT manifest_version,manifest FROM generations WHERE game_id=? AND id=?",
        )
        .bind(&self.game)
        .bind(id)
        .fetch_one(&self.tracker.pool)
        .await
        .context("Deployment generation is unavailable")?;
        ensure!(
            matches!(version, 1..=2),
            "This generation requires a compatible Deployd version"
        );
        let manifest: super::manifest::Manifest = serde_json::from_str(&document)?;
        ensure!(
            manifest.game_id == self.game
                && i64::from(manifest.version) == version
                && manifest.id()? == id,
            "Historical manifest is damaged"
        );
        let objects = manifest.objects();
        let references: Vec<(String, i64)> = sqlx::query_as("SELECT r.sha256,o.size FROM generation_object_references r JOIN generation_objects o ON o.game_id=r.game_id AND o.sha256=r.sha256 WHERE r.game_id=? AND r.generation_id=?").bind(&self.game).bind(id).fetch_all(&self.tracker.pool).await?;
        let references = references
            .into_iter()
            .map(|(hash, size)| Ok((hash, u64::try_from(size)?)))
            .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
        ensure!(
            references == objects,
            "Historical content references are incomplete; restoration is unavailable"
        );
        let dependency: Option<(String,String)> = sqlx::query_as("SELECT family_id,revision_id FROM generation_shared_dependencies WHERE game_id=? AND generation_id=?")
            .bind(&self.game).bind(id).fetch_optional(&self.tracker.pool).await?;
        ensure!(
            dependency == manifest.shared_revision,
            "Historical shared references are incomplete"
        );
        if let Some((family, revision)) = &dependency {
            super::shared::load(self, family, revision, control.clone()).await?;
        }
        let store = self.store.clone();
        self.lease
            .blocking(move || -> Result<()> {
                for (sha256, size) in objects {
                    store.verify(&Identity { sha256, size }, &control)?;
                }
                Ok(())
            })
            .await
            .context("History verification worker stopped")??;
        Ok(manifest)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) async fn materialize(
        &self,
        identity: Identity,
        destination: PathBuf,
        control: Control,
    ) -> Result<()> {
        let store = self.store.clone();
        self.lease
            .blocking(move || store.materialize(&identity, &destination, 0o600, &control))
            .await
            .context("Restoration worker stopped")?
    }

    pub(super) async fn finish_deletions(&self) -> Result<()> {
        let objects: Vec<(String, i64)> = sqlx::query_as("SELECT d.sha256,o.size FROM generation_deletions d JOIN generation_objects o ON o.game_id=d.game_id AND o.sha256=d.sha256 WHERE d.game_id=?").bind(&self.game).fetch_all(&self.tracker.pool).await?;
        for (hash, size) in objects {
            let store = self.store.clone();
            let identity = Identity {
                sha256: hash.clone(),
                size: size.try_into()?,
            };
            self.lease
                .blocking(move || store.remove(&identity))
                .await
                .context("History deletion worker stopped")??;
            let mut tx = durable(&self.tracker).await?;
            sqlx::query("DELETE FROM generation_objects WHERE game_id=? AND sha256=?")
                .bind(&self.game)
                .bind(hash)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(())
    }
}
