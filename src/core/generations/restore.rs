use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::game::GameEngine;

use super::catalog::{History, durable};
use super::content::{self, Control};
use super::manifest::Manifest;
use super::records::{self, Table};
use super::target::relative;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    id: String,
    generation: String,
    profile: String,
    name: String,
    mods: BTreeMap<String, String>,
    plugins: BTreeMap<String, String>,
    groups: BTreeMap<String, String>,
}

impl Intent {
    fn marker(&self) -> String {
        format!(".deployd-restoration-{}", self.id)
    }

    fn create(manifest: &Manifest, name: &str) -> Result<Self> {
        ensure!(!name.trim().is_empty(), "A restored profile needs a name");
        let identifiers = |table| -> Result<BTreeMap<String, String>> {
            manifest
                .records
                .iter()
                .filter(|rows| rows.table == table)
                .flat_map(|rows| &rows.rows)
                .map(|row| {
                    Ok((
                        records::text(row, "id")?.to_owned(),
                        uuid::Uuid::new_v4().to_string(),
                    ))
                })
                .collect()
        };
        Ok(Self {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            generation: manifest.id()?,
            profile: uuid::Uuid::new_v4().to_string(),
            name: name.trim().to_owned(),
            mods: identifiers(Table::Mods)?,
            plugins: identifiers(Table::Plugins)?,
            groups: identifiers(Table::Groups)?,
        })
    }

    fn suffix<'a>(&self, logical: &'a str) -> Result<(&str, &'a Path)> {
        let source = logical
            .strip_prefix("cache/")
            .context("Unsupported restored source anchor")?;
        let (old, suffix) = source.split_once('/').unwrap_or((source, ""));
        let new = self
            .mods
            .get(old)
            .context("Restored source refers to an unknown mod")?;
        if suffix.is_empty() {
            return Ok((new, Path::new("")));
        }
        Ok((new, relative(suffix)?))
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && uuid::Uuid::parse_str(&self.id).is_ok()
                && uuid::Uuid::parse_str(&self.profile).is_ok(),
            "Unsupported restoration journal"
        );
        for id in self
            .mods
            .values()
            .chain(self.groups.values())
            .chain(self.plugins.values())
        {
            uuid::Uuid::parse_str(id).context("Invalid restored identity")?;
        }
        Ok(())
    }
}

pub(super) async fn restore(
    history: &History,
    generation: &str,
    name: &str,
    control: Control,
) -> Result<String> {
    let manifest = history
        .load_with_control(generation, control.clone())
        .await?;
    ensure!(
        manifest.engine != GameEngine::MassEffect,
        "MELE restoration is unavailable until retained-output activation is connected"
    );
    let intent = Intent::create(&manifest, name)?;
    let mut tx = durable(&history.tracker).await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM profiles WHERE game_id=? AND name=?)")
            .bind(&history.game)
            .bind(&intent.name)
            .fetch_one(&mut *tx)
            .await?;
    ensure!(!exists, "A profile with that name already exists");
    sqlx::query("INSERT INTO generation_journals(id,game_id,generation_id,kind,document_version,document) VALUES (?,?,?,'restore',1,?)").bind(&intent.id).bind(&history.game).bind(generation).bind(serde_json::to_string(&intent)?).execute(&mut *tx).await.context("Finish the pending operation before restoring another profile")?;
    tx.commit().await?;
    let result = restore_inner(history, &manifest, &intent, control).await;
    match result {
        Ok(()) => {
            finish(history, &manifest, &intent, true).await?;
            Ok(intent.profile)
        }
        Err(error) => match finish(history, &manifest, &intent, false).await {
            Ok(()) => Err(error),
            Err(recovery) => {
                Err(error.context(format!("Restoration recovery is pending: {recovery:#}")))
            }
        },
    }
}

async fn restore_inner(
    history: &History,
    manifest: &Manifest,
    intent: &Intent,
    control: Control,
) -> Result<()> {
    let cache = history.cache.clone();
    let store = history.store.clone();
    let document = serde_json::to_string(intent)?;
    let sources = manifest.sources.clone();
    let worker_control = control.clone();
    history
        .lease
        .blocking(move || -> Result<()> {
            let control = worker_control;
            let intent: Intent = serde_json::from_str(&document)?;
            intent.validate()?;
            for id in intent.mods.values() {
                control.check()?;
                let root = cache.join(id);
                ensure!(
                    !root.try_exists()?,
                    "Restoration destination '{}' is occupied",
                    root.display()
                );
                let temporary = tempfile::Builder::new()
                    .prefix(".restore-profile-")
                    .tempdir_in(&cache)?;
                fs::write(temporary.path().join(intent.marker()), &intent.id)?;
                fs::File::open(temporary.path().join(intent.marker()))?.sync_all()?;
                fs::File::open(temporary.path())?.sync_all()?;
                fs::rename(temporary.path(), &root)?;
                fs::File::open(&cache)?.sync_all()?;
            }
            for source in sources {
                control.check()?;
                let (new, suffix) = intent.suffix(&source.path)?;
                ensure!(
                    suffix != Path::new(&intent.marker()),
                    "Restored content conflicts with operation metadata"
                );
                let destination = cache.join(new).join(suffix);
                if let Some(identity) = source.content {
                    fs::create_dir_all(
                        destination
                            .parent()
                            .context("Restored file has no parent")?,
                    )?;
                    store.materialize(&identity, &destination, source.mode | 0o200, &control)?;
                } else {
                    fs::create_dir_all(&destination)?;
                    fs::set_permissions(
                        &destination,
                        fs::Permissions::from_mode(source.mode | 0o700),
                    )?;
                    fs::File::open(&destination)?.sync_all()?;
                }
            }
            Ok(())
        })
        .await
        .context("Profile materialization worker stopped")??;
    control.check()?;
    let mut tx = durable(&history.tracker).await?;
    for rows in &manifest.records {
        for original in &rows.rows {
            let mut row = original.clone();
            for (column, mapping) in [
                ("mod_id", &intent.mods),
                ("plugin_id", &intent.plugins),
                ("group_id", &intent.groups),
            ] {
                if let Some(value) = row.get(column).and_then(Value::as_str) {
                    row.insert(
                        column.into(),
                        Value::String(
                            mapping
                                .get(value)
                                .with_context(|| {
                                    format!("Historical {column} reference is incomplete")
                                })?
                                .clone(),
                        ),
                    );
                }
            }
            if row.contains_key("profile_id") {
                row.insert("profile_id".into(), Value::String(intent.profile.clone()));
            }
            match rows.table {
                Table::Profiles => {
                    row.insert("id".into(), Value::String(intent.profile.clone()));
                    row.insert("name".into(), Value::String(intent.name.clone()));
                    row.insert("is_active".into(), Value::from(0));
                    row.insert("save_mode".into(), Value::String("profile".into()));
                }
                Table::Mods => {
                    row.insert(
                        "id".into(),
                        Value::String(
                            intent
                                .mods
                                .get(records::text(original, "id")?)
                                .context("Historical identity is missing")?
                                .clone(),
                        ),
                    );
                    row.insert("enabled".into(), Value::from(0));
                }
                Table::Plugins => {
                    row.insert(
                        "id".into(),
                        Value::String(
                            intent
                                .plugins
                                .get(records::text(original, "id")?)
                                .context("Historical identity is missing")?
                                .clone(),
                        ),
                    );
                    row.insert("enabled".into(), Value::from(0));
                }
                Table::Groups => {
                    row.insert(
                        "id".into(),
                        Value::String(
                            intent
                                .groups
                                .get(records::text(original, "id")?)
                                .context("Historical identity is missing")?
                                .clone(),
                        ),
                    );
                }
                Table::Files => {
                    let (id, suffix) = intent.suffix(records::text(original, "cache_path")?)?;
                    row.insert(
                        "cache_path".into(),
                        Value::String(
                            history
                                .cache
                                .join(id)
                                .join(suffix)
                                .to_str()
                                .context("Restored cache path is not UTF-8")?
                                .to_owned(),
                        ),
                    );
                }
                _ => {}
            }
            records::insert(&mut tx, rows.table, &row).await?;
        }
    }
    let mut restored = manifest.clone();
    restored.records = records::capture(&mut tx, &history.game, &intent.profile).await?;
    for rows in &mut restored.records {
        if rows.table == Table::Files {
            for row in &mut rows.rows {
                let physical = Path::new(records::text(row, "cache_path")?);
                let suffix = physical
                    .strip_prefix(&history.cache)
                    .context("Restored file escaped its cache")?;
                row.insert(
                    "cache_path".into(),
                    Value::String(format!(
                        "cache/{}",
                        suffix.to_str().context("Restored path is not UTF-8")?
                    )),
                );
            }
        }
    }
    for source in &mut restored.sources {
        let (id, suffix) = intent.suffix(&source.path)?;
        let path = if suffix.as_os_str().is_empty() {
            format!("cache/{id}")
        } else {
            format!(
                "cache/{id}/{}",
                suffix.to_str().context("Restored path is not UTF-8")?
            )
        };
        source.path = path;
        source.mode |= if source.content.is_some() {
            0o200
        } else {
            0o700
        };
    }
    restored
        .sources
        .sort_by(|left, right| left.path.cmp(&right.path));
    sqlx::query("INSERT INTO generation_drafts(profile_id,game_id,generation_id,fingerprint,seed_live_saves) VALUES (?,?,?,?,1)").bind(&intent.profile).bind(&history.game).bind(&intent.generation).bind(restored.fingerprint()?).execute(&mut *tx).await?;
    sqlx::query("UPDATE generation_journals SET committed=1 WHERE id=? AND game_id=?")
        .bind(&intent.id)
        .bind(&history.game)
        .execute(&mut *tx)
        .await?;
    control.check()?;
    tx.commit().await.context("Cannot commit restored profile")
}

async fn finish(
    history: &History,
    manifest: &Manifest,
    intent: &Intent,
    committed: bool,
) -> Result<()> {
    intent.validate()?;
    let stored: Option<(String, bool)> = sqlx::query_as(
        "SELECT document,committed FROM generation_journals WHERE id=? AND game_id=?",
    )
    .bind(&intent.id)
    .bind(&history.game)
    .fetch_optional(&history.tracker.pool)
    .await?;
    ensure!(
        stored == Some((serde_json::to_string(intent)?, committed)),
        "Restoration commit decision changed; recovery is blocked"
    );
    let cache = history.cache.clone();
    let sources = manifest.sources.clone();
    let document = serde_json::to_string(intent)?;
    history
        .lease
        .blocking(move || -> Result<()> {
            let intent: Intent = serde_json::from_str(&document)?;
            for id in intent.mods.values() {
                let root = cache.join(id);
                if !root.try_exists()? {
                    ensure!(!committed, "A restored mod is missing before cleanup");
                    continue;
                }
                ensure!(
                    fs::symlink_metadata(&root)?.is_dir(),
                    "Restoration destination changed; recovery stopped"
                );
                let marker = root.join(intent.marker());
                if committed && !marker.try_exists()? {
                    continue;
                }
                ensure!(
                    fs::read_to_string(&marker)? == intent.id,
                    "Restoration ownership marker changed; files were preserved"
                );
                if committed {
                    fs::remove_file(marker)?;
                    fs::File::open(&root)?.sync_all()?;
                    continue;
                }
                let mut expected = BTreeMap::new();
                for source in &sources {
                    let (new, suffix) = intent.suffix(&source.path)?;
                    if new == id && !suffix.as_os_str().is_empty() {
                        expected.insert(suffix.to_owned(), source);
                    }
                }
                let mut directories = BTreeSet::new();
                for path in expected.keys() {
                    let mut parent = path.parent();
                    while let Some(path) = parent {
                        directories.insert(path.to_owned());
                        parent = path.parent();
                    }
                }
                let mut entries = Vec::new();
                for entry in walkdir::WalkDir::new(&root)
                    .min_depth(1)
                    .follow_links(false)
                    .contents_first(true)
                {
                    let entry = entry?;
                    let suffix = entry.path().strip_prefix(&root)?;
                    if suffix == Path::new(&intent.marker()) {
                        continue;
                    }
                    if entry.file_type().is_dir() {
                        ensure!(
                            directories.contains(suffix)
                                || expected
                                    .get(suffix)
                                    .is_some_and(|source| source.content.is_none()),
                            "Unrecognized restored directory was preserved"
                        );
                    } else {
                        let source = expected
                            .get(suffix)
                            .context("Unrecognized restored file was preserved")?;
                        let identity = source
                            .content
                            .as_ref()
                            .context("Restored directory was replaced by a file")?;
                        ensure!(
                            &content::inspect(entry.path(), &Control::default())? == identity,
                            "External edits to a restored mod were preserved; recovery is blocked"
                        );
                    }
                    entries.push((entry.path().to_owned(), entry.file_type().is_dir()));
                }
                for (path, directory) in entries {
                    if directory {
                        fs::remove_dir(path)?;
                    } else {
                        fs::remove_file(path)?;
                    }
                }
                fs::remove_file(marker)?;
                fs::remove_dir(root)?;
                fs::File::open(&cache)?.sync_all()?;
            }
            Ok(())
        })
        .await
        .context("Restoration cleanup worker stopped")??;
    let mut tx = durable(&history.tracker).await?;
    sqlx::query("DELETE FROM generation_journals WHERE id=? AND committed=?")
        .bind(&intent.id)
        .bind(committed)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(super) async fn recover(history: &History) -> Result<()> {
    let pending: Option<(String, bool)> = sqlx::query_as(
        "SELECT document,committed FROM generation_journals WHERE game_id=? AND kind='restore'",
    )
    .bind(&history.game)
    .fetch_optional(&history.tracker.pool)
    .await?;
    if let Some((document, committed)) = pending {
        let intent: Intent = serde_json::from_str(&document)?;
        intent.validate()?;
        let manifest = history.load(&intent.generation).await?;
        finish(history, &manifest, &intent, committed).await?;
    }
    Ok(())
}
