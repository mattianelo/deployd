use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::tracker::Tracker;
use crate::models::game::Game;

use super::catalog::durable;
use super::content::Control;
use super::operation::Lease;

const PREFIX: &str = "game_removal_";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cleanup {
    version: u32,
    game: String,
    cache: PathBuf,
    data: PathBuf,
    mods: Vec<String>,
    profiles: Vec<String>,
    sources: BTreeSet<String>,
    launcher_sources: BTreeSet<String>,
    family: Option<i64>,
    location: Option<i64>,
}

impl Cleanup {
    fn key(&self) -> String {
        format!("{PREFIX}{}", self.game)
    }

    fn directories(&self) -> Result<Vec<PathBuf>> {
        ensure!(self.version == 1, "Unsupported game-removal cleanup record");
        let history = crate::utils::paths::generation_store_in(&self.cache, &self.game)?;
        let mut paths = vec![
            history,
            self.data.join(&self.game).join("vanilla-backup"),
            self.data.join("mele-originals").join(&self.game),
            self.data.join("mele-transactions").join(&self.game),
            self.data.join("wine-prefixes").join(&self.game),
        ];
        for id in &self.mods {
            ensure!(
                !id.is_empty()
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
                "Invalid mod cache identity; cleanup was stopped"
            );
            paths.push(self.cache.join(id));
        }
        for (folder, hashes) in [
            ("mele-sources", &self.sources),
            ("mele-launcher-sources", &self.launcher_sources),
        ] {
            for hash in hashes {
                ensure!(
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "Invalid retained-source identity; cleanup was stopped"
                );
                paths.push(self.data.join(folder).join(hash));
            }
        }
        if let Some(family) = self.family {
            ensure!(family > 0, "Invalid launcher family identity");
            paths.push(
                self.data
                    .join("mele-family-originals")
                    .join(family.to_string()),
            );
            paths.push(
                self.data
                    .join("mele-family-transactions")
                    .join(family.to_string()),
            );
        }
        Ok(paths)
    }

    fn check(&self, protected: &[PathBuf]) -> Result<()> {
        for root in [&self.cache, &self.data] {
            let metadata = fs::symlink_metadata(root).with_context(|| {
                format!("Managed storage '{}' is unavailable; restore access before retrying full reset", root.display())
            })?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "Managed storage '{}' was replaced; cleanup was stopped",
                root.display()
            );
        }
        let mut boundaries = self.directories()?;
        if self.game.starts_with("mass-effect-le") {
            boundaries.push(crate::utils::paths::mele_result_cache_in(&self.data));
        }
        check_boundaries(&boundaries, protected)
    }

    fn clean(&self, protected: &[PathBuf]) -> Result<()> {
        self.check(protected)?;
        crate::core::game::mass_effect::forget_results(&self.data, &self.game, &self.profiles)?;
        remove_named_links(&self.cache, &self.mods)?;
        for path in self.directories()? {
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    ensure!(
                        metadata.is_dir() && !metadata.file_type().is_symlink(),
                        "Managed storage '{}' was replaced; cleanup was stopped",
                        path.display()
                    );
                    fs::remove_dir_all(&path).with_context(|| format!("Cannot remove managed storage '{}'; restore cache access and retry full reset", path.display()))?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Cannot inspect managed storage '{}'", path.display())
                    });
                }
            }
        }
        Ok(())
    }
}

fn check_boundaries(paths: &[PathBuf], protected: &[PathBuf]) -> Result<()> {
    for path in paths {
        check_ancestors(path)?;
        let candidate = absolute(path)?;
        for keep in protected {
            let keep = absolute(keep)?;
            ensure!(
                !keep.starts_with(&candidate) && !candidate.starts_with(&keep),
                "Cleanup overlaps a game, save, prefix or downloaded archive at '{}'; move it out of the managed cache before retrying",
                keep.display()
            );
        }
    }
    Ok(())
}

pub(crate) async fn delete_mod_cache(tracker: &Tracker, cache: &Path, id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
        "Invalid mod cache identity"
    );
    let protected = protected_paths(tracker, &crate::utils::paths::deployd_data_dir()?).await?;
    let path = cache.join(id);
    tokio::task::spawn_blocking(move || {
        check_boundaries(std::slice::from_ref(&path), &protected)?;
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("Cannot remove mod cache"),
        }
    })
    .await
    .context("Mod cache cleanup worker stopped")?
}

async fn protected_paths(tracker: &Tracker, data: &Path) -> Result<Vec<PathBuf>> {
    let mut protected: Vec<PathBuf> = sqlx::query_scalar::<_, String>(
        "SELECT path FROM games WHERE path IS NOT NULL UNION SELECT wine_prefix FROM games WHERE wine_prefix IS NOT NULL UNION SELECT archive_path FROM download_entries WHERE archive_path IS NOT NULL AND archive_path<>''",
    ).fetch_all(&tracker.pool).await?.into_iter().map(PathBuf::from).collect();
    protected.push(data.join("saves"));
    Ok(protected)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.try_exists()? {
        return fs::canonicalize(path).context("Cannot verify cleanup boundaries");
    }
    let parent = path.parent().context("Cleanup path has no parent")?;
    let name = path.file_name().context("Cleanup path has no filename")?;
    if path.is_absolute() {
        return Ok(absolute(parent)?.join(name));
    }
    absolute(&std::env::current_dir()?.join(path))
}

fn check_ancestors(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && path
                .components()
                .all(|c| !matches!(c, Component::ParentDir)),
        "Cleanup requires an absolute managed-storage path"
    );
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "Managed storage contains a symbolic-link ancestor at '{}'; cleanup was stopped",
                ancestor.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot inspect cleanup access at '{}'", ancestor.display())
                });
            }
        }
    }
    Ok(())
}

fn remove_named_links(cache: &Path, mods: &[String]) -> Result<()> {
    for directory in [cache.join("named_mods"), cache.join("named_mods/by-name")] {
        check_ancestors(&directory)?;
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("Cannot inspect named mod folders"),
        };
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                let target = fs::read_link(entry.path())?;
                let target = if target.is_absolute() {
                    target
                } else {
                    directory.join(target)
                };
                if mods.iter().any(|id| target == cache.join(id)) {
                    fs::remove_file(entry.path()).context("Cannot remove a named mod link")?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn remove(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    control: Control,
) -> Result<()> {
    remove_in(
        tracker,
        game,
        cache,
        crate::utils::paths::deployd_data_dir()?,
        control,
    )
    .await
}

pub(super) async fn remove_in(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    data: PathBuf,
    control: Control,
) -> Result<()> {
    if tracker
        .get_setting(&format!("{PREFIX}{}", game.id))
        .await?
        .is_some()
    {
        return resume(tracker, &game.id).await;
    }
    let ids = crate::core::tracker::removal::scope(&game.id);
    let mut members = vec![(game.clone(), cache.to_path_buf())];
    for record in tracker.load_games(true).await? {
        if record.id != game.id && ids.contains(&record.id.as_str()) {
            let member_cache = match tracker
                .get_setting(&format!("cache_dir_{}", record.id))
                .await?
            {
                Some(path) => PathBuf::from(path),
                None => crate::utils::paths::cache_root()?,
            };
            members.push((
                crate::core::location_recovery::persisted_game(record),
                member_cache,
            ));
        }
    }
    members.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    for (member, _) in &members {
        tracker.ensure_location_ready(&member.id).await?;
    }
    let mut launcher_sources = std::collections::BTreeMap::new();
    for (member, _) in &members {
        let hashes: Vec<String> = sqlx::query_scalar("SELECT json_extract(m.value,'$.source_sha256') FROM mele_families f, json_each(f.document,'$.mods') m WHERE json_extract(m.value,'$.owner')=?")
            .bind(&member.id).fetch_all(&tracker.pool).await?;
        launcher_sources.insert(member.id.clone(), hashes);
    }
    (control.phase)("Purging deployment before removal…");
    let mut lease = None;
    for (member, cache) in &members {
        let history = super::activation::purge_for_removal(
            tracker, member, cache, data.clone(), control.clone(), lease.clone(),
        ).await.with_context(|| format!("Could not purge {}. All games remain managed; retry full reset to finish the remaining purges", member.title))?;
        lease = Some(history.lease.clone());
    }
    let lease = lease.context("No games selected for removal")?;
    let mutation = std::sync::Arc::new(
        crate::core::game::mass_effect::mutation_lock()
            .lock_owned()
            .await,
    );
    control.check()?;
    (control.phase)("Removing game management data…");
    let protected = protected_paths(tracker, &data).await?;
    let mut tx = durable(tracker).await?;
    let mut cleanups = Vec::new();
    for (member, cache) in &members {
        let mut cleanup = inventory(&mut tx, member, cache, data.clone(), &ids).await?;
        if let Some(hashes) = launcher_sources.remove(&member.id) {
            cleanup.sources.extend(hashes);
            cleanup.launcher_sources = cleanup.sources.clone();
        }
        cleanups.push(cleanup);
    }
    for cleanup in &cleanups {
        reset_records(&mut tx, cleanup).await?;
    }
    for cleanup in &mut cleanups {
        preserve_shared_sources(&mut tx, cleanup).await?;
    }
    let checking = cleanups.clone();
    lease
        .blocking(move || {
            for cleanup in checking {
                cleanup.check(&protected)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Cleanup preflight worker stopped")??;
    tx.rollback().await?;
    let mut tx = durable(tracker).await?;
    let document = serde_json::to_string(&cleanups)?;
    for cleanup in &cleanups {
        sqlx::query("UPDATE games SET hidden=0 WHERE id=?")
            .bind(&cleanup.game)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO settings(key,value) VALUES (?,?)")
            .bind(cleanup.key())
            .bind(&document)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit()
        .await
        .context("Cannot commit game reset; cleanup was not started")?;
    finish(tracker, cleanups, &lease, mutation).await
}

async fn inventory(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    game: &Game,
    cache: &Path,
    data: PathBuf,
    ids: &[&str],
) -> Result<Cleanup> {
    let launcher_sources: Vec<String> = sqlx::query_scalar("SELECT json_extract(m.value,'$.source_sha256') FROM mele_families f, json_each(f.document,'$.mods') m WHERE json_extract(m.value,'$.owner')=? UNION SELECT json_extract(m.value,'$.source_sha256') FROM generation_shared_revisions r, json_each(r.manifest,'$.mods') m WHERE json_extract(m.value,'$.owner')=?")
        .bind(&game.id).bind(&game.id).fetch_all(&mut **tx).await?;
    let mods = sqlx::query_scalar("SELECT id FROM mods WHERE game_id=?")
        .bind(&game.id)
        .fetch_all(&mut **tx)
        .await?;
    let profiles = sqlx::query_scalar("SELECT id FROM profiles WHERE game_id=? UNION SELECT originating_profile_id FROM generations WHERE game_id=?")
        .bind(&game.id).bind(&game.id).fetch_all(&mut **tx).await?;
    let mut cleanup = Cleanup {
        version: 1,
        game: game.id.clone(),
        cache: cache.into(),
        data,
        mods,
        profiles,
        sources: launcher_sources.into_iter().collect(),
        launcher_sources: BTreeSet::new(),
        family: None,
        location: None,
    };
    let documents: Vec<String> = sqlx::query_scalar("SELECT document FROM mele_packages WHERE mod_id IN (SELECT id FROM mods WHERE game_id=?) UNION ALL SELECT manifest FROM generations WHERE game_id=?")
        .bind(&game.id).bind(&game.id).fetch_all(&mut **tx).await?;
    for document in documents {
        let value: serde_json::Value = serde_json::from_str(&document)?;
        source_hashes(&value, &mut cleanup.sources);
        if value.get("records").is_some() {
            let manifest: super::manifest::Manifest = serde_json::from_value(value)?;
            manifest.validate()?;
            ensure!(
                manifest.game_id == game.id,
                "Historical mod cache belongs to another game"
            );
            for rows in manifest
                .records
                .iter()
                .filter(|rows| rows.table == super::records::Table::MelePackages)
            {
                for row in &rows.rows {
                    let record = serde_json::from_str(super::records::text(row, "document")?)?;
                    source_hashes(&record, &mut cleanup.sources);
                }
            }
            for rows in manifest
                .records
                .iter()
                .filter(|rows| rows.table == super::records::Table::Mods)
            {
                for row in &rows.rows {
                    let id = super::records::text(row, "id")?;
                    let shared: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM mods WHERE id=? AND game_id<>?)",
                    )
                    .bind(id)
                    .bind(&game.id)
                    .fetch_one(&mut **tx)
                    .await?;
                    if !shared && !cleanup.mods.iter().any(|current| current == id) {
                        cleanup.mods.push(id.into());
                    }
                }
            }
        }
    }
    let family: Option<i64> = sqlx::query_scalar("SELECT location_id FROM game_locations WHERE game_id=? AND role='game' AND location_id IN (SELECT location_id FROM mele_families)")
        .bind(&game.id).fetch_optional(&mut **tx).await?;
    cleanup.location = family;
    if let Some(family) = family {
        let peers: Vec<String> = sqlx::query_scalar(
            "SELECT game_id FROM game_locations WHERE location_id=? AND role='game'",
        )
        .bind(family)
        .fetch_all(&mut **tx)
        .await?;
        if peers.iter().all(|peer| ids.contains(&peer.as_str())) {
            cleanup.family = Some(family);
        }
    }
    cleanup.launcher_sources = cleanup.sources.clone();
    Ok(cleanup)
}

async fn reset_records(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    cleanup: &Cleanup,
) -> Result<()> {
    crate::core::tracker::removal::reset(tx, &cleanup.game).await?;
    for statement in [
        "DELETE FROM generation_shared_objects WHERE game_id=?",
        "DELETE FROM generation_objects WHERE game_id=?",
        "DELETE FROM generation_stores WHERE game_id=?",
    ] {
        sqlx::query(statement)
            .bind(&cleanup.game)
            .execute(&mut **tx)
            .await?;
    }
    if let Some(family) = cleanup.family {
        sqlx::query("DELETE FROM generation_shared_state WHERE family_id=?")
            .bind(family.to_string())
            .execute(&mut **tx)
            .await?;
        sqlx::query("DELETE FROM mele_families WHERE location_id=?")
            .bind(family)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query("DELETE FROM generation_shared_revisions WHERE family_id=? AND NOT EXISTS(SELECT 1 FROM generation_shared_dependencies d WHERE d.family_id=generation_shared_revisions.family_id AND d.revision_id=generation_shared_revisions.id) AND NOT EXISTS(SELECT 1 FROM generation_shared_state s WHERE s.family_id=generation_shared_revisions.family_id AND s.revision_id=generation_shared_revisions.id)").bind(cleanup.location.map(|id| id.to_string()).unwrap_or_default()).execute(&mut **tx).await?;
    Ok(())
}

async fn preserve_shared_sources(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    cleanup: &mut Cleanup,
) -> Result<()> {
    for hash in cleanup.sources.clone() {
        let used: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mele_packages WHERE instr(document,?)>0 UNION ALL SELECT 1 FROM generations WHERE instr(manifest,?)>0 UNION ALL SELECT 1 FROM mele_families WHERE instr(document,?)>0 UNION ALL SELECT 1 FROM generation_shared_revisions WHERE instr(manifest,?)>0)")
            .bind(&hash).bind(&hash).bind(&hash).bind(&hash).fetch_one(&mut **tx).await?;
        if used {
            cleanup.sources.remove(&hash);
            cleanup.launcher_sources.remove(&hash);
        }
    }
    Ok(())
}

fn source_hashes(value: &serde_json::Value, hashes: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(values) => {
            if let Some(hash) = values
                .get("source_sha256")
                .and_then(serde_json::Value::as_str)
            {
                hashes.insert(hash.into());
            }
            for value in values.values() {
                source_hashes(value, hashes);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                source_hashes(value, hashes);
            }
        }
        _ => {}
    }
}

async fn finish(
    tracker: &Tracker,
    mut cleanups: Vec<Cleanup>,
    lease: &std::sync::Arc<Lease>,
    mutation: std::sync::Arc<tokio::sync::OwnedMutexGuard<()>>,
) -> Result<()> {
    let first = cleanups.first().context("Empty game cleanup")?;
    let protected = protected_paths(tracker, &first.data).await?;
    let mut tx = durable(tracker).await?;
    for cleanup in &cleanups {
        sqlx::query("DELETE FROM settings WHERE key=?")
            .bind(cleanup.key())
            .execute(&mut *tx)
            .await?;
        reset_records(&mut tx, cleanup).await?;
    }
    for cleanup in &mut cleanups {
        preserve_shared_sources(&mut tx, cleanup).await?;
    }
    tx.rollback().await?;
    let deleting = cleanups.clone();
    let held = mutation.clone();
    lease
        .blocking(move || {
            let _mutation = held;
            for cleanup in &deleting {
                cleanup.check(&protected)?;
            }
            for cleanup in deleting {
                cleanup.clean(&protected)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("Game cleanup worker stopped")??;
    let mut tx = durable(tracker).await?;
    for cleanup in &cleanups {
        sqlx::query("DELETE FROM settings WHERE key=?")
            .bind(cleanup.key())
            .execute(&mut *tx)
            .await?;
        reset_records(&mut tx, cleanup).await?;
        sqlx::query("DELETE FROM game_locations WHERE game_id=?")
            .bind(&cleanup.game)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM settings WHERE key=?")
            .bind(format!("cache_dir_{}", cleanup.game))
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE games SET hidden=1,title=NULL,path=NULL,data_subdir='Data',engine=NULL,wine_prefix=NULL,custom=0 WHERE id=?")
            .bind(&cleanup.game).execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM folder_locations WHERE NOT EXISTS(SELECT 1 FROM game_locations g WHERE g.location_id=folder_locations.id) AND NOT EXISTS(SELECT 1 FROM location_repairs r WHERE r.location_id=folder_locations.id) AND NOT EXISTS(SELECT 1 FROM mele_families f WHERE f.location_id=folder_locations.id) AND NOT EXISTS(SELECT 1 FROM mele_profile_groups p WHERE p.location_id=folder_locations.id)").execute(&mut *tx).await?;
    tx.commit()
        .await
        .context("Cannot finish game removal; retry full reset")
}

pub(crate) async fn resume(tracker: &Tracker, game: &str) -> Result<()> {
    let lease = Lease::acquire().await?;
    let mutation = std::sync::Arc::new(
        crate::core::game::mass_effect::mutation_lock()
            .lock_owned()
            .await,
    );
    if let Some(document) = tracker.get_setting(&format!("{PREFIX}{game}")).await? {
        let cleanups: Vec<Cleanup> =
            serde_json::from_str(&document).context("Invalid pending game cleanup")?;
        ensure!(
            cleanups.iter().any(|cleanup| cleanup.game == game)
                && cleanups
                    .iter()
                    .all(|cleanup| crate::core::tracker::removal::scope(game)
                        .contains(&cleanup.game.as_str())),
            "Pending cleanup belongs to another game"
        );
        finish(tracker, cleanups, &lease, mutation).await?;
    }
    Ok(())
}

pub(crate) async fn resume_all(tracker: &Tracker) -> Result<Vec<String>> {
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT min(key) FROM settings WHERE substr(key,1,13)='game_removal_' GROUP BY value",
    )
    .fetch_all(&tracker.pool)
    .await?;
    let mut warnings = Vec::new();
    for key in keys {
        if let Some(game) = key.strip_prefix(PREFIX)
            && let Err(error) = resume(tracker, game).await
        {
            warnings.push(format!("Game reset for {game} needs cleanup: {error:#}. Restore cache access and choose Purge & delete all Deployd data again."));
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests;
