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
    pub(super) id: String,
    pub(super) game: String,
    pub(super) changes: Vec<Change>,
}

pub(super) struct Applied {
    id: String,
    game: String,
    document: String,
}

impl Applied {
    pub(super) async fn decide(self, tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
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
    pub(super) async fn prepare(
        history: &History,
        game: &Game,
        desired: Vec<(Target, Node)>,
        control: Control,
    ) -> Result<Self> {
        let mut changes = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (target, after) in desired {
            ensure!(
                seen.insert(target.resolve(game)?.to_string_lossy().to_lowercase()),
                "Duplicate activation destination"
            );
            after.validate()?;
            let path = target.resolve(game)?;
            let read = path.clone();
            let read_control = control.clone();
            let before = tokio::task::spawn_blocking(move || inspect(&read, &read_control))
                .await
                .context("Activation inspection worker stopped")??;
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
                    &history.retain(path, control.clone()).await? == identity,
                    "Managed file changed during preparation"
                );
            }
            changes.push(Change {
                target,
                before,
                after,
            });
        }
        Ok(Self {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            game: game.id.clone(),
            changes,
        })
    }

    pub(super) fn validate(&self, game: &Game) -> Result<()> {
        ensure!(
            self.version == 1 && self.game == game.id && uuid::Uuid::parse_str(&self.id).is_ok(),
            "Unsupported or invalid activation journal"
        );
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
            kind == "deploy" || kind == "purge",
            "Unsupported activation operation"
        );
        let mut objects = protected;
        for change in &self.changes {
            for node in [&change.before, &change.after] {
                if let Node::File { identity, .. } = node {
                    objects.insert(identity.sha256.clone(), identity.size);
                }
            }
        }
        let store = history.store.clone();
        let verify = objects.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            for (sha256, size) in verify {
                store.verify(&Identity { sha256, size }, &Control::default())?;
            }
            Ok(())
        })
        .await
        .context("Journal content verification worker stopped")??;
        history.register(&objects).await?;
        let mut tx = durable(&history.tracker).await?;
        sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,?,1,?)").bind(&self.id).bind(&self.game).bind(kind).bind(serde_json::to_string(self)?).execute(&mut *tx).await.context("Another operation needs recovery before activation")?;
        for (hash, _) in objects {
            sqlx::query("INSERT INTO generation_pending_objects(operation_id,game_id,sha256) VALUES (?,?,?)").bind(&self.id).bind(&self.game).bind(hash).execute(&mut *tx).await?;
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
        let journal = self.clone();
        let store = history.store.clone();
        let game = game.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            for change in &journal.changes {
                control.check()?;
                let path = change.target.resolve(&game)?;
                parents(&path, &game)?;
                ensure!(
                    inspect(&path, &control)? == change.before,
                    "Managed file changed since preparation; activation stopped: {}",
                    path.display()
                );
                if change.before != change.after {
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

    async fn check_record(&self, history: &History, committed: bool) -> Result<()> {
        ensure!(
            self.game == history.game,
            "Journal and history store target different games"
        );
        let stored: Option<(String, bool)> = sqlx::query_as(
            "SELECT document,committed FROM generation_journals WHERE id=? AND game_id=?",
        )
        .bind(&self.id)
        .bind(&self.game)
        .fetch_optional(&history.tracker.pool)
        .await?;
        ensure!(
            stored == Some((serde_json::to_string(self)?, committed)),
            "Durable recovery decision or intent differs; dependent operations are blocked"
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
        let journal = self.clone();
        let store = history.store.clone();
        let game = game.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let control = Control::default();
            for change in journal.changes.iter().rev() {
                let path = change.target.resolve(&game)?;
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
