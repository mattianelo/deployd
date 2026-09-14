use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::models::game::{Game, GameEngine};

use super::catalog::History;
use super::content::{Control, Identity};
use super::records::{self, Rows, Table};
use super::target::{Target, relative};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub(super) path: String,
    pub(super) content: Option<Identity>,
    pub(super) mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Output {
    pub(super) target: Target,
    pub(super) content: Option<Identity>,
    pub(super) mode: u32,
    pub(super) mod_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) version: u32,
    pub(super) game_id: String,
    pub(super) engine: GameEngine,
    pub(super) records: Vec<Rows>,
    pub(super) sources: Vec<Source>,
    pub(super) outputs: Vec<Output>,
    pub(super) base_inputs: Vec<Output>,
    pub(super) shared_revision: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mele: Option<crate::core::game::mass_effect::generations::Snapshot>,
}

impl Manifest {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.version, 1..=2) && self.records.len() == Table::ALL.len(),
            "Unsupported or incomplete deployment generation"
        );
        for (rows, table) in self.records.iter().zip(Table::ALL) {
            ensure!(rows.table == table, "Historical metadata order is invalid");
        }
        ensure!(
            self.records[0].rows.len() == 1,
            "Historical profile metadata is missing"
        );
        let profile = &self.records[0].rows[0];
        ensure!(
            records::text(profile, "game_id")? == self.game_id,
            "Historical profile belongs to another game"
        );
        crate::utils::paths::generation_store_in(Path::new("."), &self.game_id)?;
        if let Some(snapshot) = &self.mele {
            ensure!(
                self.version >= 2 && self.engine == GameEngine::MassEffect,
                "MELE snapshot requires its versioned engine manifest"
            );
            snapshot.validate(&self.game_id)?;
            ensure!(
                self.base_inputs.is_empty(),
                "MELE vanilla inputs must use its engine snapshot"
            );
            ensure!(
                self.outputs.len() == snapshot.files.len(),
                "Historical MELE output inventory is incomplete"
            );
            for file in &snapshot.files {
                ensure!(
                    self.outputs.iter().any(|output| output.target
                        == (Target::MassEffect {
                            path: file.relative.clone()
                        })
                        && output
                            .content
                            .as_ref()
                            .is_some_and(|identity| identity.sha256 == file.sha256
                                && identity.size == file.size)
                        && output.mod_id.is_none()),
                    "Historical MELE output differs from its recipe result"
                );
            }
        }
        if let Some((family, revision)) = &self.shared_revision {
            ensure!(
                self.engine == GameEngine::MassEffect
                    && family.parse::<i64>().is_ok_and(|id| id > 0),
                "Invalid shared launcher dependency"
            );
            Identity {
                size: 0,
                sha256: revision.clone(),
            }
            .validate()?;
        }
        let mut objects = BTreeMap::new();
        let mut paths = BTreeMap::new();
        for file in &self.sources {
            relative(&file.path)?;
            ensure!(
                paths
                    .insert(file.path.as_str(), file.content.is_none())
                    .is_none(),
                "Duplicate historical source path"
            );
            ensure!(
                file.mode & !0o777 == 0,
                "Invalid historical file permissions"
            );
            if let Some(identity) = &file.content {
                identity.validate()?;
                if let Some(size) = objects.insert(&identity.sha256, identity.size) {
                    ensure!(
                        size == identity.size,
                        "Historical content has inconsistent sizes"
                    );
                }
            }
        }
        for path in paths.keys() {
            if let Some((parent, _)) = path.rsplit_once('/')
                && parent != "cache"
                && parent != "mele-sources"
            {
                ensure!(
                    paths.get(parent) == Some(&true),
                    "Historical inventory is missing a parent directory"
                );
            }
        }
        let mut targets = BTreeSet::new();
        for file in &self.outputs {
            file.target.validate(&self.engine)?;
            ensure!(
                file.mode & !0o777 == 0,
                "Invalid historical output permissions"
            );
            ensure!(
                targets.insert(&file.target),
                "Duplicate historical deployment target"
            );
            if let Some(identity) = &file.content {
                identity.validate()?;
            }
        }
        let mut inputs = BTreeSet::new();
        for file in &self.base_inputs {
            file.target.validate(&self.engine)?;
            ensure!(
                inputs.insert(&file.target),
                "Duplicate historical base input"
            );
            ensure!(
                file.mode & !0o777 == 0,
                "Invalid historical base permissions"
            );
            if let Some(identity) = &file.content {
                identity.validate()?;
            }
        }
        super::validation::metadata(self)
    }

    pub(super) fn id(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }

    pub(super) fn fingerprint(&self) -> Result<String> {
        Ok(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&self.records, &self.sources))?)
        ))
    }

    pub(super) fn profile(&self) -> Result<(&str, &str)> {
        let row = &self
            .records
            .first()
            .context("Historical profile is absent")?
            .rows
            .first()
            .context("Historical profile is absent")?;
        Ok((records::text(row, "id")?, records::text(row, "name")?))
    }

    pub(super) fn objects(&self) -> BTreeMap<String, u64> {
        self.sources
            .iter()
            .filter_map(|file| file.content.as_ref())
            .chain(self.outputs.iter().filter_map(|file| file.content.as_ref()))
            .map(|id| (id.sha256.clone(), id.size))
            .collect()
    }
}

type Inventory = BTreeMap<String, (PathBuf, (u64, u64, u64, i64, i64))>;

pub(super) async fn capture(
    history: &History,
    game: &Game,
    profile: &str,
    data: PathBuf,
    control: Control,
) -> Result<Manifest> {
    let mut tx = history.tracker.pool.begin().await?;
    let records = records::capture(&mut tx, &game.id, profile).await?;
    tx.commit().await?;
    let cache = history.cache.clone();
    let game = game.clone();
    let store = history.store.clone();
    let manifest = history
        .lease
        .blocking(move || -> Result<Manifest> {
            let mut manifest = Manifest {
                version: if game.engine == GameEngine::MassEffect {
                    2
                } else {
                    1
                },
                game_id: game.id.clone(),
                engine: game.engine.clone(),
                records,
                sources: Vec::new(),
                outputs: Vec::new(),
                base_inputs: Vec::new(),
                shared_revision: None,
                mele: None,
            };
            let mut roots = BTreeMap::new();
            for rows in &manifest.records {
                if rows.table == Table::Mods && game.engine != GameEngine::MassEffect {
                    for row in &rows.rows {
                        let id = records::text(row, "id")?;
                        ensure!(!id.contains('/'), "Invalid historical mod identity");
                        relative(id)?;
                        roots.insert(format!("cache/{id}"), cache.join(id));
                    }
                }
                if rows.table == Table::MelePackages {
                    for row in &rows.rows {
                        let record: crate::core::game::mass_effect::library::Record =
                            serde_json::from_str(records::text(row, "document")?)?;
                        record.validate()?;
                        let hash = record.package.source_sha256;
                        Identity {
                            size: 0,
                            sha256: hash.clone(),
                        }
                        .validate()?;
                        let id = records::text(row, "mod_id")?;
                        relative(id)?;
                        ensure!(!id.contains('/'), "Invalid MELE mod identity");
                        let writable = cache.join(id);
                        roots.insert(
                            format!("cache/{id}"),
                            if record.writable_cache {
                                writable
                            } else {
                                data.join("mele-sources").join(hash)
                            },
                        );
                    }
                }
            }
            let inventory = |roots: &BTreeMap<String, PathBuf>| -> Result<Inventory> {
                let mut sources = BTreeMap::new();
                for (logical, root) in roots {
                    ensure!(
                        root.is_dir(),
                        "Mod source '{}' is unavailable; restore access before deploying",
                        root.display()
                    );
                    for entry in walkdir::WalkDir::new(root)
                        .follow_links(false)
                        .sort_by_file_name()
                    {
                        control.check()?;
                        let entry = entry.context("Cannot inventory complete mod version")?;
                        ensure!(
                            entry.file_type().is_file() || entry.file_type().is_dir(),
                            "Mod inventory contains a symbolic link or special file: {}",
                            entry.path().display()
                        );
                        let suffix = entry.path().strip_prefix(root)?;
                        let key = if suffix.as_os_str().is_empty() {
                            logical.clone()
                        } else {
                            format!(
                                "{logical}/{}",
                                suffix.to_str().context("Mod path is not UTF-8")?
                            )
                        };
                        relative(&key)?;
                        let m = fs::symlink_metadata(entry.path())?;
                        sources.insert(
                            key,
                            (
                                entry.path().to_owned(),
                                (m.dev(), m.ino(), m.len(), m.ctime(), m.ctime_nsec()),
                            ),
                        );
                    }
                }
                Ok(sources)
            };
            let sources = inventory(&roots)?;
            for (logical, (source, _)) in &sources {
                control.check()?;
                let metadata = fs::symlink_metadata(source)?;
                let content = if metadata.is_dir() {
                    None
                } else {
                    Some(store.retain(source, &control)?)
                };
                manifest.sources.push(Source {
                    path: logical.clone(),
                    content,
                    mode: metadata.permissions().mode() & 0o777,
                });
            }
            ensure!(
                sources == inventory(&roots)?,
                "Mod inventory changed during preparation; close tools and retry"
            );
            for rows in &mut manifest.records {
                if rows.table != Table::Files {
                    continue;
                }
                for row in &mut rows.rows {
                    Target::file(&game.engine, records::text(row, "game_rel_original")?)?;
                    let source = Path::new(records::text(row, "cache_path")?);
                    let mod_root = format!("cache/{}", records::text(row, "mod_id")?);
                    let logical = sources
                        .iter()
                        .find(|(logical, (path, _))| {
                            path.as_path() == source
                                && (logical.as_str() == mod_root
                                    || logical.starts_with(&format!("{mod_root}/")))
                        })
                        .map(|(logical, _)| logical.clone())
                        .with_context(|| {
                            format!(
                                "Tracked file '{}' is missing from its complete mod inventory",
                                source.display()
                            )
                        })?;
                    row.insert("cache_path".into(), Value::String(logical));
                }
            }
            manifest.validate()?;
            Ok(manifest)
        })
        .await
        .context("Complete mod inventory worker stopped")??;
    history.register(&manifest.objects()).await?;
    Ok(manifest)
}
