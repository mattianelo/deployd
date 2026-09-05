use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::utils::location::{
    FolderChange, FolderRole, FolderSelection, SelectedLocation, resolve_relative,
};

use super::Tracker;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundFolder {
    pub(crate) game_id: String,
    pub(crate) title: String,
    pub(crate) role: FolderRole,
    pub(crate) relative: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct LocationRecord {
    pub(crate) id: i64,
    pub(crate) selection: SelectedLocation,
    pub(crate) bindings: Vec<BoundFolder>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingRecovery {
    pub(crate) location_id: i64,
    pub(crate) changes: Vec<FolderChange>,
}

pub(super) async fn migrate(pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;
    for sql in [
        "CREATE TABLE IF NOT EXISTS folder_locations (id INTEGER PRIMARY KEY, root TEXT NOT NULL, host_hint TEXT)",
        "CREATE TABLE IF NOT EXISTS game_locations (game_id TEXT NOT NULL, role TEXT NOT NULL CHECK(role IN ('game','prefix')), location_id INTEGER NOT NULL REFERENCES folder_locations(id), relative_path TEXT NOT NULL, PRIMARY KEY(game_id, role))",
        "CREATE TABLE IF NOT EXISTS location_repairs (location_id INTEGER PRIMARY KEY REFERENCES folder_locations(id), changes TEXT NOT NULL)",
        "CREATE INDEX IF NOT EXISTS idx_game_locations_root ON game_locations(location_id)",
        "CREATE TRIGGER IF NOT EXISTS remove_game_locations AFTER DELETE ON games BEGIN DELETE FROM game_locations WHERE game_id = OLD.id; END",
    ] {
        sqlx::query(sql)
            .execute(&mut *tx)
            .await
            .context("Failed to upgrade folder recovery storage")?;
    }
    let rows = sqlx::query("SELECT id, path, wine_prefix FROM games WHERE path IS NOT NULL")
        .fetch_all(&mut *tx)
        .await?;
    for row in rows {
        let id: String = row.try_get("id")?;
        for (role, column) in [
            (FolderRole::Game, "path"),
            (FolderRole::Prefix, "wine_prefix"),
        ] {
            let path: Option<String> = row.try_get(column)?;
            sync_binding(&mut tx, &id, role, path.as_deref().map(Path::new), None).await?;
        }
    }
    tx.commit()
        .await
        .context("Failed to commit folder recovery upgrade")
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .context("Folder recovery cannot store a path with invalid UTF-8")
}

pub(crate) async fn sync_binding(
    connection: &mut SqliteConnection,
    game_id: &str,
    role: FolderRole,
    path: Option<&Path>,
    selected: Option<&FolderSelection>,
) -> Result<()> {
    let existing = sqlx::query("SELECT l.id, l.root, b.relative_path FROM game_locations b JOIN folder_locations l ON l.id=b.location_id WHERE b.game_id=? AND b.role=?")
        .bind(game_id).bind(role.key()).fetch_optional(&mut *connection).await?;
    if let Some(row) = &existing {
        let root: String = row.try_get("root")?;
        let relative: String = row.try_get("relative_path")?;
        let unchanged = path.is_some_and(|path| {
            resolve_relative(Path::new(&root), Path::new(&relative)).is_ok_and(|old| old == path)
        });
        if unchanged && selected.is_none() {
            return Ok(());
        }
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM location_repairs WHERE location_id=?)")
                .bind(row.try_get::<i64, _>("id")?)
                .fetch_one(&mut *connection)
                .await?;
        if pending {
            bail!("Restore folder access must finish before changing this game's folders")
        }
        if unchanged
            && let Some(selected) = selected
            && selected.location.root == Path::new(&root)
            && selected.relative == Path::new(&relative)
        {
            sqlx::query("UPDATE folder_locations SET host_hint=COALESCE(?, host_hint) WHERE id=?")
                .bind(
                    selected
                        .location
                        .host_hint
                        .as_deref()
                        .map(path_text)
                        .transpose()?,
                )
                .bind(row.try_get::<i64, _>("id")?)
                .execute(&mut *connection)
                .await?;
            return Ok(());
        }
    }
    let Some(path) = path else {
        sqlx::query("DELETE FROM game_locations WHERE game_id=? AND role=?")
            .bind(game_id)
            .bind(role.key())
            .execute(connection)
            .await?;
        return Ok(());
    };
    let fallback = FolderSelection {
        role,
        location: SelectedLocation {
            root: path.to_path_buf(),
            host_hint: None,
        },
        relative: PathBuf::new(),
    };
    let selected = selected.unwrap_or(&fallback);
    if selected.role != role
        || resolve_relative(&selected.location.root, &selected.relative)? != path
    {
        bail!("The selected folder does not match the game's location binding");
    }
    let root = path_text(&selected.location.root)?;
    let hint = selected
        .location
        .host_hint
        .as_deref()
        .map(path_text)
        .transpose()?;
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM folder_locations WHERE root=? AND NOT EXISTS(SELECT 1 FROM location_repairs WHERE location_id=folder_locations.id) ORDER BY id LIMIT 1")
        .bind(root).fetch_optional(&mut *connection).await?;
    let id = match id {
        Some(id) => {
            sqlx::query("UPDATE folder_locations SET host_hint=COALESCE(?, host_hint) WHERE id=?")
                .bind(hint)
                .bind(id)
                .execute(&mut *connection)
                .await?;
            id
        }
        None => sqlx::query("INSERT INTO folder_locations(root,host_hint) VALUES (?,?)")
            .bind(root)
            .bind(hint)
            .execute(&mut *connection)
            .await?
            .last_insert_rowid(),
    };
    sqlx::query("INSERT INTO game_locations(game_id,role,location_id,relative_path) VALUES (?,?,?,?) ON CONFLICT(game_id,role) DO UPDATE SET location_id=excluded.location_id, relative_path=excluded.relative_path")
        .bind(game_id).bind(role.key()).bind(id).bind(path_text(&selected.relative)?).execute(connection).await?;
    Ok(())
}

async fn read_location(connection: &mut SqliteConnection, id: i64) -> Result<LocationRecord> {
    let row = sqlx::query("SELECT root,host_hint FROM folder_locations WHERE id=?")
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
    let root: String = row.try_get("root")?;
    let hint: Option<String> = row.try_get("host_hint")?;
    let rows = sqlx::query("SELECT b.game_id,b.role,b.relative_path,COALESCE(g.title,b.game_id) AS title FROM game_locations b JOIN games g ON g.id=b.game_id WHERE b.location_id=? ORDER BY b.game_id,b.role")
        .bind(id).fetch_all(connection).await?;
    let bindings = rows
        .into_iter()
        .map(|row| {
            Ok(BoundFolder {
                game_id: row.try_get("game_id")?,
                title: row.try_get("title")?,
                role: FolderRole::parse(row.try_get::<&str, _>("role")?)?,
                relative: PathBuf::from(row.try_get::<String, _>("relative_path")?),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(LocationRecord {
        id,
        selection: SelectedLocation {
            root: root.into(),
            host_hint: hint.map(Into::into),
        },
        bindings,
    })
}

async fn rebase_tool_paths(
    connection: &mut SqliteConnection,
    game_id: &str,
    old: &Path,
    new: &Path,
) -> Result<()> {
    let rows = sqlx::query("SELECT id,exe_path,working_dir FROM tools WHERE game_id=?")
        .bind(game_id)
        .fetch_all(&mut *connection)
        .await?;
    for row in rows {
        for column in ["exe_path", "working_dir"] {
            let value: Option<String> = row.try_get(column)?;
            let Some(value) = value else { continue };
            if let Some(rebased) = crate::utils::location::rebase(Path::new(&value), old, new)? {
                sqlx::query(&format!("UPDATE tools SET {column}=? WHERE id=?"))
                    .bind(path_text(&rebased)?)
                    .bind(row.try_get::<String, _>("id")?)
                    .execute(&mut *connection)
                    .await?;
            }
        }
    }
    Ok(())
}

impl Tracker {
    pub(crate) async fn folder_location(
        &self,
        game_id: &str,
        role: FolderRole,
    ) -> Result<LocationRecord> {
        let mut connection = self.pool.acquire().await?;
        let id: i64 = sqlx::query_scalar(
            "SELECT location_id FROM game_locations WHERE game_id=? AND role=?",
        )
        .bind(game_id)
        .bind(role.key())
        .fetch_optional(&mut *connection)
        .await?
        .context(
            "This folder is not saved yet. Save the game configuration before restoring access",
        )?;
        read_location(&mut connection, id).await
    }

    pub(crate) async fn backfill_location_hints(&self) -> Result<()> {
        let rows = sqlx::query("SELECT id,root FROM folder_locations WHERE host_hint IS NULL AND EXISTS(SELECT 1 FROM game_locations WHERE location_id=folder_locations.id)").fetch_all(&self.pool).await?;
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let root: String = row.try_get("root")?;
            let selected = SelectedLocation::capture(PathBuf::from(&root)).await;
            if let Some(hint) = selected.host_hint {
                sqlx::query("UPDATE folder_locations SET host_hint=? WHERE id=? AND root=? AND host_hint IS NULL")
                    .bind(path_text(&hint)?).bind(id).bind(root).execute(&self.pool).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn commit_location_recovery(
        &self,
        previous: &LocationRecord,
        selected: &SelectedLocation,
        confirmed_original: bool,
    ) -> Result<PendingRecovery> {
        if !selected.validate_identity(&previous.selection)? && !confirmed_original {
            bail!("Confirm that this is the original folder before restoring access");
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = read_location(&mut tx, previous.id).await?;
        if current.selection != previous.selection || current.bindings != previous.bindings {
            bail!(
                "Folder settings changed while the picker was open. Start Restore folder access again"
            );
        }
        let pending: Option<String> =
            sqlx::query_scalar("SELECT changes FROM location_repairs WHERE location_id=?")
                .bind(previous.id)
                .fetch_optional(&mut *tx)
                .await?;
        if current.bindings.is_empty() {
            bail!("No games use this location anymore")
        }
        let mut changes = Vec::new();
        for binding in &current.bindings {
            let old_path = resolve_relative(&current.selection.root, &binding.relative)?;
            let new_path = resolve_relative(&selected.root, &binding.relative)?;
            let sql = match binding.role {
                FolderRole::Game => "UPDATE games SET path=? WHERE id=? AND path=?",
                FolderRole::Prefix => "UPDATE games SET wine_prefix=? WHERE id=? AND wine_prefix=?",
            };
            let updated = sqlx::query(sql)
                .bind(path_text(&new_path)?)
                .bind(&binding.game_id)
                .bind(path_text(&old_path)?)
                .execute(&mut *tx)
                .await?;
            if updated.rows_affected() != 1 {
                bail!("Game configuration no longer matches its folder binding")
            }
            rebase_tool_paths(&mut tx, &binding.game_id, &old_path, &new_path).await?;
            changes.push(FolderChange {
                game_id: binding.game_id.clone(),
                role: binding.role,
                old_path,
                new_path,
            });
        }
        if let Some(pending) = pending {
            let earlier: Vec<FolderChange> = serde_json::from_str(&pending)?;
            for mut earlier in earlier {
                let current = changes
                    .iter()
                    .find(|change| {
                        change.game_id == earlier.game_id
                            && change.role == earlier.role
                            && change.old_path == earlier.new_path
                    })
                    .context("The pending repair does not match the current folder bindings")?;
                earlier.new_path = current.new_path.clone();
                if !changes.contains(&earlier) {
                    changes.push(earlier);
                }
            }
        }
        sqlx::query(
            "UPDATE folder_locations SET root=?,host_hint=COALESCE(?,host_hint) WHERE id=?",
        )
        .bind(path_text(&selected.root)?)
        .bind(selected.host_hint.as_deref().map(path_text).transpose()?)
        .bind(previous.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO location_repairs(location_id,changes) VALUES (?,?) ON CONFLICT(location_id) DO UPDATE SET changes=excluded.changes")
            .bind(previous.id).bind(serde_json::to_string(&changes)?).execute(&mut *tx).await?;
        tx.commit()
            .await
            .context("Could not commit folder recovery; previous settings were retained")?;
        Ok(PendingRecovery {
            location_id: previous.id,
            changes,
        })
    }

    pub(crate) async fn pending_location_repairs(&self) -> Result<Vec<PendingRecovery>> {
        let rows =
            sqlx::query("SELECT location_id,changes FROM location_repairs ORDER BY location_id")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PendingRecovery {
                    location_id: row.try_get("location_id")?,
                    changes: serde_json::from_str(row.try_get("changes")?)?,
                })
            })
            .collect()
    }

    pub(crate) async fn ensure_location_ready(&self, game_id: &str) -> Result<()> {
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM location_repairs r JOIN game_locations b ON b.location_id=r.location_id WHERE b.game_id=?)")
            .bind(game_id).fetch_one(&self.pool).await?;
        if pending {
            bail!(
                "Folder repair is incomplete. Open Manage Games → Restore folder access to retry before using this game"
            )
        }
        Ok(())
    }

    pub(crate) async fn finish_location_repair(&self, pending: &PendingRecovery) -> Result<()> {
        let removed = sqlx::query("DELETE FROM location_repairs WHERE location_id=? AND changes=?")
            .bind(pending.location_id)
            .bind(serde_json::to_string(&pending.changes)?)
            .execute(&self.pool)
            .await?;
        if removed.rows_affected() != 1 {
            bail!("The folder repair changed before completion; retry recovery")
        }
        Ok(())
    }
}
