use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, Transaction};

use crate::models::game::Game;

use super::catalog::{History, durable};
use super::content::{self, Control, Identity};
use super::target::Target;

mod cache;
mod layout;
mod mele;
mod shared;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Node {
    Absent,
    Directory { mode: u32 },
    File { identity: Identity, mode: u32 },
}

impl Node {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Absent => Ok(()),
            Self::Directory { mode } | Self::File { mode, .. } => {
                ensure!(mode & !0o777 == 0, "Invalid journal file permissions");
                if let Self::File { identity, .. } = self {
                    identity.validate()?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    pub(super) target: Target,
    pub(super) before: Node,
    pub(super) after: Node,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cache_sources: Vec<(Target, String)>,
    pub(super) id: String,
    pub(super) game: String,
    pub(super) changes: Vec<Change>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    saves: Option<crate::core::save_manager::activation::Transition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    directories: Vec<layout::Directory>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<layout::Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mele: Option<crate::core::game::mass_effect::journal::Journal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) shared: Option<super::shared::Intent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) dependency: Option<crate::core::game::mass_effect::family::generations::Revision>,
}

pub(super) struct Applied {
    id: String,
    game: String,
    document: String,
}

impl Applied {
    pub(super) async fn commit(
        self,
        history: &History,
        game: &Game,
        previous: Option<&super::state::State>,
        deployment: Option<&super::state::Deployment<'_>>,
        saves: &crate::core::save_manager::SaveSetId,
    ) -> Result<()> {
        ensure!(
            self.game == game.id && self.game == history.game,
            "Activation belongs to another game"
        );
        let journal: Journal = serde_json::from_str(&self.document)?;
        journal.validate_request(game, previous, deployment, saves)?;
        if let Some(transition) = &journal.saves {
            verify_saves(history, game, transition).await?;
        }
        let mut tx = durable(&history.tracker).await?;
        ensure!(
            super::state::read(&mut tx, &game.id).await?.as_ref() == previous,
            "Deployed state or live save ownership changed after preparation"
        );
        if deployment.is_none()
            && let Some(previous) = previous
        {
            ensure!(
                &previous.saves == saves,
                "Purge must preserve live save ownership"
            );
        }
        let kind: String =
            sqlx::query_scalar("SELECT kind FROM generation_journals WHERE id=? AND game_id=?")
                .bind(&self.id)
                .bind(&self.game)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            kind == if deployment.is_some() {
                "deploy"
            } else {
                "purge"
            },
            "Activation operation differs from its durable intent"
        );
        journal.verify_mele(history, game, true).await?;
        journal.verify_dependency(history, game).await?;
        mele::verify_inputs(
            history,
            game,
            deployment,
            journal.mele.as_ref().map(|mele| &mele.desired),
        )
        .await?;
        let mele = journal.mele.clone();
        let dependency = journal.dependency.clone();
        let saves_to_verify = journal.saves.clone();
        let verified_game = game.clone();
        let base_inputs = deployment
            .map(|deployment| deployment.manifest.base_inputs.clone())
            .unwrap_or_default();
        history.lease.blocking(move || -> Result<()> {
            journal.validate(&verified_game)?;
            journal.verify_links(&verified_game)?;
            journal.verify_directories(&verified_game, true)?;
            for input in &base_inputs {
                let actual = if let Some(change) = journal.changes.iter().find(|change| change.target == input.target) {
                    change.before.clone()
                } else {
                    layout::accessible(&verified_game, &input.target, &input.target.resolve(&verified_game)?, false)?;
                    inspect(&input.target.resolve(&verified_game)?, &Control::default())?
                };
                ensure!(match (&input.content, actual) {
                    (Some(expected), Node::File {identity, ..}) => expected == &identity,
                    (None, Node::Directory {..}) => true,
                    _ => false,
                }, "Required base inputs changed; explicitly prepare a new generation against the current game");
            }
            for change in &journal.changes {
                let path = change.target.resolve(&verified_game)?;
                layout::accessible(&verified_game, &change.target, &path, false)?;
                parents(&path, &verified_game)?;
                ensure!(inspect(&path, &Control::default())? == change.after, "Managed files changed before commitment; recovery information was preserved");
            }
            Ok(())
        }).await.context("Final activation verification worker stopped")??;
        if let Some(mele) = mele {
            mele.generation_commit(&mut tx).await?;
        }
        if deployment.is_some()
            && let Some(dependency) = dependency
        {
            super::shared::commit_dependency(&mut tx, &game.id, &dependency).await?;
        }
        super::state::publish(&mut tx, history, game, &self.id, deployment, saves).await?;
        if let Some(saves) = saves_to_verify {
            verify_saves(history, game, &saves).await?;
        }
        self.decide(&mut tx).await?;
        tx.commit()
            .await
            .context("Activation commitment failed; recover its durable decision before retrying")
    }

    async fn decide(self, tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        let changed = sqlx::query("UPDATE generation_journals SET committed=1 WHERE id=? AND game_id=? AND document=? AND committed=0").bind(self.id).bind(self.game).bind(self.document).execute(&mut **tx).await?;
        ensure!(
            changed.rows_affected() == 1,
            "Activation recovery record changed before commitment"
        );
        Ok(())
    }
}

pub(super) fn inspect(path: &Path, control: &Control) -> Result<Node> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Node::Absent),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Cannot inspect managed path '{}'", path.display()));
        }
    };
    let mode = metadata.permissions().mode() & 0o777;
    if metadata.is_dir() {
        return Ok(Node::Directory { mode });
    }
    ensure!(
        metadata.is_file(),
        "A managed path is a symbolic link or special file; it was preserved: {}",
        path.display()
    );
    Ok(Node::File {
        identity: content::inspect(path, control)?,
        mode,
    })
}

fn parents(path: &Path, game: &Game) -> Result<()> {
    let mut parent = path.parent();
    let roots = [game.path.clone(), crate::core::game::deploy_dir(game)];
    while let Some(path) = parent {
        if roots.iter().any(|root| root == path) {
            break;
        }
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("Deployment parent '{}' is unavailable", path.display()))?;
        ensure!(
            metadata.is_dir(),
            "Deployment parent is not a directory: {}",
            path.display()
        );
        parent = path.parent();
    }
    Ok(())
}

impl Journal {
    pub(super) fn has_saves(&self) -> bool {
        self.saves.is_some()
    }

    pub(super) fn validate_request(
        &self,
        game: &Game,
        previous: Option<&super::state::State>,
        deployment: Option<&super::state::Deployment<'_>>,
        saves: &crate::core::save_manager::SaveSetId,
    ) -> Result<()> {
        self.validate(game)?;
        self.validate_mele_request(deployment)?;
        ensure!(
            self.shared.is_none(),
            "Game activation cannot publish shared changes"
        );
        if let Some(deployment) = deployment {
            ensure!(
                super::shared::identity(self.dependency.as_ref())?
                    == deployment.manifest.shared_revision,
                "Game activation differs from its recorded shared dependency"
            );
        }
        ensure!(
            saves.game_id() == game.id,
            "Save ownership belongs to another game"
        );
        ensure!(
            deployment.is_some() || self.saves.is_none(),
            "Purge cannot switch live saves"
        );
        ensure!(
            previous.is_some() || !crate::core::game::has_save_management(game),
            "Initialize live save ownership before preparing activation"
        );
        if let Some(transition) = &self.saves {
            ensure!(
                &transition.target == saves,
                "Save participant targets a different save owner"
            );
            if let Some(previous) = previous {
                ensure!(
                    previous.saves == transition.source,
                    "Save participant has stale source ownership"
                );
            }
        } else if let Some(previous) = previous {
            ensure!(
                &previous.saves == saves,
                "Changing save ownership requires an applied save participant"
            );
        }
        if let Some(deployment) = deployment {
            for output in &deployment.manifest.outputs {
                let expected = match &output.content {
                    Some(identity) => Node::File {
                        identity: identity.clone(),
                        mode: output.mode,
                    },
                    None => Node::Directory { mode: output.mode },
                };
                ensure!(
                    self.changes
                        .iter()
                        .any(|change| change.target == self.physical(&output.target)
                            && change.after == expected),
                    "A generation output was not verified by this activation"
                );
            }
        }
        Ok(())
    }

    pub(super) async fn verify_prepared(
        &self,
        history: &History,
        game: &Game,
        inputs: Vec<super::manifest::Output>,
        control: Control,
    ) -> Result<()> {
        self.verify_mele(history, game, false).await?;
        self.verify_dependency(history, game).await?;
        self.verify_shared(history, game, false).await?;
        let cache = history.cache.clone();
        let store = history.store.clone();
        let journal = self.clone();
        let game = game.clone();
        history.lease.blocking(move || -> Result<()> {
            journal.validate(&game)?;
            journal.verify_cache(&cache, &store, &control)?;
            journal.verify_links(&game)?;
            journal.verify_directories(&game, false)?;
            for change in &journal.changes {
                control.check()?;
                let path = change.target.resolve(&game)?;
                journal.prepared_parents(&game, &change.target)?;
                ensure!(inspect(&path, &control)? == change.before,
                    "Managed files changed after preparation; prepare deployment again");
            }
            for input in inputs {
                layout::accessible(&game, &input.target, &input.target.resolve(&game)?, false)?;
                let actual = inspect(&input.target.resolve(&game)?, &control)?;
                ensure!(match (input.content, actual) {
                    (Some(expected), Node::File { identity, .. }) => expected == identity,
                    (None, Node::Directory { .. }) => true,
                    _ => false,
                }, "Required base inputs changed; explicitly prepare a new generation against the current game");
            }
            Ok(())
        }).await.context("Prepared activation verification worker stopped")?
    }

    pub(super) fn from_record(version: i64, document: &str) -> Result<Self> {
        let journal: Self =
            serde_json::from_str(document).context("Invalid deployment recovery journal")?;
        ensure!(
            i64::from(journal.version) == version,
            "Recovery journal versions disagree; records were preserved"
        );
        Ok(journal)
    }

    pub(super) async fn prepare(
        history: &History,
        game: &Game,
        desired: Vec<(Target, Node)>,
        control: Control,
    ) -> Result<Self> {
        ensure!(
            history.game == game.id,
            "Activation preparation belongs to another game"
        );
        control.check()?;
        let store = history.store.clone();
        let game = game.clone();
        let preparing = control.clone();
        let (journal, objects) = history
            .lease
            .blocking(move || -> Result<_> {
                let mut journal = Self {
                    version: 1,
                    cache_sources: Vec::new(),
                    id: uuid::Uuid::new_v4().to_string(),
                    game: game.id.clone(),
                    changes: Vec::new(),
                    saves: None,
                    directories: Vec::new(),
                    links: Vec::new(),
                    mele: None,
                    shared: None,
                    dependency: None,
                };
                if desired
                    .iter()
                    .any(|(target, _)| matches!(target, Target::CustomIni { .. }))
                {
                    journal.capture_links(&game)?;
                }
                let mut objects = BTreeMap::new();
                let mut seen = std::collections::BTreeSet::new();
                for (target, after) in desired {
                    preparing.check()?;
                    let physical = journal.physical(&target);
                    if let Some(change) = journal
                        .changes
                        .iter()
                        .find(|change| change.target == physical)
                    {
                        ensure!(
                            journal.links.iter().any(|link| physical
                                == (Target::CustomIni {
                                    slot: link.destination
                                }))
                                && change.after == after,
                            "Duplicate or conflicting activation destination"
                        );
                        continue;
                    }
                    let target = physical;
                    let path = target.resolve(&game)?;
                    ensure!(
                        seen.insert(path.to_string_lossy().to_lowercase()),
                        "Duplicate activation destination"
                    );
                    after.validate()?;
                    layout::accessible(&game, &target, &path, true)?;
                    let before = inspect(&path, &preparing)?;
                    ensure!(
                        !matches!(
                            (&before, &after),
                            (Node::Directory { .. }, Node::File { .. })
                                | (Node::File { .. }, Node::Directory { .. })
                        ),
                        "A file/directory conflict blocks activation; resolve it before retrying"
                    );
                    if let Node::File { identity, .. } = &before {
                        ensure!(
                            store.retain(&path, &preparing)? == *identity,
                            "Managed file changed during preparation"
                        );
                        objects.insert(identity.sha256.clone(), identity.size);
                    }
                    journal.changes.push(Change {
                        target,
                        before,
                        after,
                    });
                }
                journal.capture_directories(&game)?;
                journal.validate(&game)?;
                journal.verify_links(&game)?;
                preparing.check()?;
                Ok((journal, objects))
            })
            .await
            .context("Activation preparation worker stopped")??;
        control.check()?;
        history.register(&objects).await?;
        Ok(journal)
    }

    pub(super) async fn extend(
        &mut self,
        history: &History,
        game: &Game,
        desired: Vec<(Target, Node)>,
        control: Control,
    ) -> Result<()> {
        ensure!(!self.has_saves(), "Prepare files before save transitions");
        let addition = Self::prepare(history, game, desired, control).await?;
        let mut merged = self.clone();
        merged.version = merged.version.max(addition.version);
        merged.changes.extend(addition.changes);
        for link in addition.links {
            if !merged.links.contains(&link) {
                merged.links.push(link);
            }
        }
        let game = game.clone();
        *self = history
            .lease
            .blocking(move || -> Result<Self> {
                merged.capture_directories(&game)?;
                merged.validate(&game)?;
                merged.verify_links(&game)?;
                Ok(merged)
            })
            .await
            .context("Configuration preparation worker stopped")??;
        Ok(())
    }

    pub(super) fn attach_saves(
        &mut self,
        game: &Game,
        transition: crate::core::save_manager::activation::Transition,
    ) -> Result<()> {
        ensure!(
            self.saves.is_none(),
            "Activation already has a save participant"
        );
        self.validate(game)?;
        transition.validate(&self.id, game)?;
        self.saves = Some(transition);
        self.version = self.version.max(2);
        Ok(())
    }

    pub(super) fn validate(&self, game: &Game) -> Result<()> {
        ensure!(
            matches!(self.version, 1..=5)
                && self.game == game.id
                && uuid::Uuid::parse_str(&self.id).is_ok(),
            "Unsupported or invalid activation journal"
        );
        if let Some(saves) = &self.saves {
            ensure!(
                self.version >= 2,
                "Save participant requires its versioned recovery format"
            );
            saves.validate(&self.id, game)?;
        }
        self.validate_cache()?;
        self.validate_layout(game)?;
        self.validate_mele(game)?;
        self.validate_shared(game)?;
        if let Some(dependency) = &self.dependency {
            ensure!(
                self.version >= 4 && self.mele.is_some() && self.shared.is_none(),
                "Unsupported shared dependency participant"
            );
            dependency.validate()?;
        }
        let mut seen = std::collections::BTreeSet::new();
        for change in &self.changes {
            let destination = change.target.resolve(game)?;
            ensure!(
                seen.insert(destination.to_string_lossy().to_lowercase()),
                "Duplicate journal destination"
            );
            change.before.validate()?;
            change.after.validate()?;
        }
        Ok(())
    }

    pub(super) async fn persist(
        &self,
        history: &History,
        game: &Game,
        kind: &str,
        protected: BTreeMap<String, u64>,
    ) -> Result<()> {
        self.validate(game)?;
        ensure!(
            self.game == history.game,
            "Journal and history store target different games"
        );
        ensure!(
            matches!(kind, "deploy" | "purge" | "shared"),
            "Unsupported activation operation"
        );
        ensure!(
            (kind == "shared") == self.shared.is_some(),
            "Shared Apply requires its separate journal kind"
        );
        ensure!(
            self.shared.is_some()
                || !self
                    .changes
                    .iter()
                    .any(|change| matches!(change.target, Target::MeleLauncher { .. })),
            "Launcher targets require a separate shared participant"
        );
        let mut objects = protected;
        for change in &self.changes {
            for node in [&change.before, &change.after] {
                if let Node::File { identity, .. } = node {
                    objects.insert(identity.sha256.clone(), identity.size);
                }
            }
        }
        if let Some(dependency) = &self.dependency {
            for identity in dependency.payloads()?.values() {
                objects.insert(identity.sha256.clone(), identity.size);
            }
        }
        let store = history.store.clone();
        let verify = objects.clone();
        history
            .lease
            .blocking(move || -> Result<()> {
                for (sha256, size) in verify {
                    store.verify(&Identity { sha256, size }, &Control::default())?;
                }
                Ok(())
            })
            .await
            .context("Journal content verification worker stopped")??;
        history.register(&objects).await?;
        let mut tx = durable(&history.tracker).await?;
        ensure!(
            kind != "purge" || self.saves.is_none(),
            "Purge cannot switch live saves"
        );
        if let Some(mele) = &self.mele {
            mele.generation_check(&mut tx, false).await?;
        }
        if let Some(shared) = &self.shared {
            shared.check(&mut tx, game, false).await?;
        }
        sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,?,?,?)").bind(&self.id).bind(&self.game).bind(kind).bind(self.version).bind(serde_json::to_string(self)?).execute(&mut *tx).await.context("Another operation needs recovery before activation")?;
        for (hash, _) in objects {
            sqlx::query("INSERT INTO generation_pending_objects(operation_id,game_id,sha256) VALUES (?,?,?)").bind(&self.id).bind(&self.game).bind(hash).execute(&mut *tx).await?;
        }
        if let Some(shared) = &self.shared {
            shared.protect(&mut tx, &self.id).await?;
        }
        tx.commit()
            .await
            .context("Cannot commit activation recovery information")
    }

    pub(super) async fn apply(
        &self,
        history: &History,
        game: &Game,
        control: Control,
    ) -> Result<Applied> {
        self.validate(game)?;
        self.check_record(history, false).await?;
        self.verify_mele(history, game, false).await?;
        self.verify_dependency(history, game).await?;
        self.verify_shared(history, game, false).await?;
        if let Some(saves) = &self.saves {
            let saves = saves.clone();
            let game = game.clone();
            history
                .lease
                .participant(async move {
                    saves.adopt_preparation(&game).await?;
                    saves.apply(&game).await
                })
                .await
                .context("Save activation participant stopped")??;
        }
        let journal = self.clone();
        let store = history.store.clone();
        let cache = history.cache.clone();
        let game = game.clone();
        history
            .lease
            .blocking(move || -> Result<()> {
                journal.verify_links(&game)?;
                for change in journal.operations(&game)? {
                    control.check()?;
                    let path = change.path;
                    journal.verify_links(&game)?;
                    layout::accessible(&game, &change.target, &path, false)?;
                    parents(&path, &game)?;
                    ensure!(
                        inspect(&path, &control)? == change.before,
                        "Managed file changed since preparation; activation stopped: {}",
                        path.display()
                    );
                    if change.before != change.after
                        && !journal.apply_cache(
                            &cache,
                            &store,
                            &change.target,
                            &path,
                            &change.after,
                            &control,
                        )?
                    {
                        apply_node(&store, &path, &change.after, &control)?;
                    }
                    ensure!(
                        inspect(&path, &control)? == change.after,
                        "Activation verification failed: {}",
                        path.display()
                    );
                }
                for change in &journal.changes {
                    let path = change.target.resolve(&game)?;
                    ensure!(
                        inspect(&path, &control)? == change.after,
                        "Managed state changed before activation commitment: {}",
                        path.display()
                    );
                }
                Ok(())
            })
            .await
            .context("Activation worker stopped")??;
        Ok(Applied {
            id: self.id.clone(),
            game: self.game.clone(),
            document: serde_json::to_string(self)?,
        })
    }

    pub(super) async fn decision(&self, history: &History) -> Result<bool> {
        ensure!(
            self.game == history.game,
            "Journal and history store target different games"
        );
        let stored: Option<(String, bool, i64)> = sqlx::query_as(
            "SELECT document,committed,document_version FROM generation_journals WHERE id=? AND game_id=?",
        )
        .bind(&self.id)
        .bind(&self.game)
        .fetch_optional(&history.tracker.pool)
        .await?;
        let (document, committed, version) = stored.context(
            "Durable activation decision is unavailable; dependent operations are blocked",
        )?;
        ensure!(
            document == serde_json::to_string(self)? && version == i64::from(self.version),
            "Durable recovery intent differs; dependent operations are blocked"
        );
        Ok(committed)
    }

    async fn check_record(&self, history: &History, committed: bool) -> Result<()> {
        ensure!(
            self.decision(history).await? == committed,
            "Durable recovery decision differs; dependent operations are blocked"
        );
        Ok(())
    }

    pub(super) async fn recover(
        &self,
        history: &History,
        game: &Game,
        committed: bool,
    ) -> Result<()> {
        self.validate(game)?;
        self.check_record(history, committed).await?;
        if let Some(shared) = &self.shared {
            shared
                .change
                .generation_root(&history.tracker, game)
                .await?;
            let mut tx = durable(&history.tracker).await?;
            shared.check(&mut tx, game, committed).await?;
            tx.rollback().await?;
        }
        if let Some(mele) = &self.mele {
            mele.generation_validate(&history.tracker, game).await?;
            let mut tx = durable(&history.tracker).await?;
            mele.generation_check(&mut tx, committed).await?;
            tx.rollback().await?;
        }
        if let Some(saves) = &self.saves {
            let saves = saves.clone();
            let game = game.clone();
            history
                .lease
                .participant(async move {
                    saves.adopt_preparation(&game).await?;
                    saves.recover(&game, committed).await
                })
                .await
                .context("Save recovery participant stopped")??;
        }
        let journal = self.clone();
        let store = history.store.clone();
        let recovery_game = game.clone();
        history.lease.blocking(move || -> Result<()> {
            let game = recovery_game;
            let control = Control::default();
            journal.verify_links(&game)?;
            for change in journal.operations(&game)?.into_iter().rev() {
                let path = change.path;
                layout::accessible(&game, &change.target, &path, !committed)?;
                let current = inspect(&path, &control)?;
                if committed {
                    ensure!(current == change.after, "Committed files changed before recovery finished; recovery information was preserved: {}", path.display());
                } else if current != change.before {
                    parents(&path, &game)?;
                    ensure!(current == change.after, "External edits prevent safe rollback; recovery information was preserved: {}", path.display());
                    apply_node(&store, &path, &change.before, &control)?;
                    ensure!(inspect(&path, &control)? == change.before, "Rollback verification failed: {}", path.display());
                }
            }
            Ok(())
        }).await.context("Activation recovery worker stopped")??;
        self.verify_mele(history, game, committed).await?;
        self.verify_shared(history, game, committed).await?;
        let mut tx = durable(&history.tracker).await?;
        sqlx::query("DELETE FROM generation_journals WHERE id=? AND game_id=? AND committed=?")
            .bind(&self.id)
            .bind(&self.game)
            .bind(committed)
            .execute(&mut *tx)
            .await?;
        tx.commit()
            .await
            .context("Cannot finish activation recovery")
    }
}

fn apply_node(
    store: &super::store::Store,
    path: &Path,
    node: &Node,
    control: &Control,
) -> Result<()> {
    match node {
        Node::Absent => match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir(path)?,
            Ok(_) => fs::remove_file(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
        Node::Directory { mode } => {
            if path.is_dir() {
                fs::set_permissions(path, fs::Permissions::from_mode(*mode))?;
                fs::File::open(path)?.sync_all()?;
            } else {
                let parent = path.parent().context("Directory target has no parent")?;
                let temporary = tempfile::Builder::new()
                    .prefix(".deployd-directory-")
                    .tempdir_in(parent)?;
                fs::set_permissions(temporary.path(), fs::Permissions::from_mode(*mode))?;
                fs::File::open(temporary.path())?.sync_all()?;
                fs::rename(temporary.path(), path)?;
            }
        }
        Node::File { identity, mode } => {
            store.materialize(identity, path, *mode, control)?;
        }
    }
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

pub(super) async fn discard_save_preparation(
    history: &History,
    game: &Game,
    operation: &str,
) -> Result<()> {
    ensure!(
        history.game == game.id,
        "Save preparation belongs to another game"
    );
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM generation_journals WHERE game_id=?")
            .bind(&game.id)
            .fetch_one(&history.tracker.pool)
            .await?;
    ensure!(
        pending == 0,
        "Finish activation recovery before discarding save preparation"
    );
    let game = game.clone();
    let operation = operation.to_owned();
    history
        .lease
        .participant(async move {
            crate::core::save_manager::activation::discard_preparation(&game, &operation).await
        })
        .await
        .context("Save preparation cleanup participant stopped")?
}

async fn verify_saves(
    history: &History,
    game: &Game,
    saves: &crate::core::save_manager::activation::Transition,
) -> Result<()> {
    let game = game.clone();
    let saves = saves.clone();
    history
        .lease
        .participant(async move { saves.verify(&game).await })
        .await
        .context("Save verification participant stopped")?
}
