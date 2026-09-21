use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::models::game::{Game, GameEngine};
use crate::utils::snap::{self, SelectedFolderKind};

use super::{Journal, Node, Target, inspect};
use crate::core::generations::content::Control;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Directory {
    target: Target,
    levels: usize,
}

impl Directory {
    fn resolve(&self, game: &Game) -> Result<PathBuf> {
        self.resolve_with(game, None)
    }

    fn resolve_with(&self, game: &Game, created: Option<&BTreeSet<PathBuf>>) -> Result<PathBuf> {
        let target = self.target.resolve_with(game, created)?;
        let path = target
            .ancestors()
            .nth(self.levels)
            .context("Invalid journal directory")?;
        let root = root(game, &self.target)?;
        ensure!(
            self.levels > 0 && path != root && path.starts_with(root),
            "Journal directory escapes its authorized root"
        );
        Ok(path.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Link {
    pub(super) slot: usize,
    pub(super) destination: usize,
}

impl Link {
    fn validate(&self, game: &Game) -> Result<()> {
        ensure!(
            game.engine == GameEngine::Bethesda && self.slot == 0 && self.destination > 0,
            "Only recognized Bethesda INI bridges can participate in activation"
        );
        Target::CustomIni {
            slot: self.destination,
        }
        .resolve(game)?;
        Ok(())
    }

    fn verify(&self, game: &Game) -> Result<()> {
        self.validate(game)?;
        let target = Target::CustomIni { slot: self.slot };
        let path = target.resolve(game)?;
        accessible(game, &target, &path, false)?;
        ensure!(
            fs::read_link(path)?
                == (Target::CustomIni {
                    slot: self.destination
                })
                .resolve(game)?,
            "The managed INI bridge changed; it and recovery records were preserved"
        );
        Ok(())
    }
}

pub(super) fn root<'a>(game: &'a Game, target: &Target) -> Result<&'a Path> {
    target.validate(&game.engine)?;
    match target {
        Target::MeleLauncher { .. } => game
            .path
            .parent()
            .context("Shared launcher root is unavailable"),
        Target::CustomIni { .. } | Target::PluginControl { .. } | Target::Eclipse { .. } => game
            .wine_prefix
            .as_deref()
            .context("Restore Wine-prefix access before deploying"),
        _ => Ok(&game.path),
    }
}

pub(super) fn accessible(game: &Game, target: &Target, path: &Path, missing: bool) -> Result<()> {
    let root = root(game, target)?;
    let kind = if game.wine_prefix.as_deref() == Some(root) {
        SelectedFolderKind::WinePrefix
    } else {
        SelectedFolderKind::GameFolder
    };
    snap::validate_readable_folder(root, kind).map_err(anyhow::Error::msg)?;
    ensure!(
        fs::symlink_metadata(root)
            .context("Authorized deployment root is unavailable; restore folder access")?
            .is_dir(),
        "Authorized deployment root is unavailable or redirected"
    );
    let _access = fs::read_dir(root).context("Restore folder access before deploying")?;
    let relative = path
        .strip_prefix(root)
        .context("Deployment target escapes its authorized root")?;
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "Invalid deployment target"
    );
    let mut current = root.to_owned();
    for part in relative.parent().into_iter().flat_map(Path::components) {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir(),
                "Deployment parent is redirected or not a directory; it was preserved: {}",
                current.display()
            ),
            Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(error)
                    .context("Deployment parent is unavailable; restore access and retry");
            }
        }
    }
    Ok(())
}

impl Journal {
    pub(in crate::core::generations) fn physical(&self, target: &Target) -> Target {
        if let Target::CustomIni { slot } = target
            && let Some(link) = self.links.iter().find(|link| link.slot == *slot)
        {
            return Target::CustomIni {
                slot: link.destination,
            };
        }
        target.clone()
    }

    pub(in crate::core::generations) fn capture_links(&mut self, game: &Game) -> Result<()> {
        let paths = crate::core::game::custom_ini_paths(game);
        if paths.is_empty() {
            return Ok(());
        }
        let target = Target::CustomIni { slot: 0 };
        accessible(game, &target, &paths[0], true)?;
        match fs::read_link(&paths[0]) {
            Ok(destination) => {
                let slot = paths.iter().position(|path| *path == destination)
                    .filter(|slot| *slot > 0).context("Unrecognized INI redirect was preserved; restore the original INI location")?;
                let link = Link {
                    slot: 0,
                    destination: slot,
                };
                link.verify(game)?;
                if !self.links.contains(&link) {
                    self.links.push(link);
                }
                self.version = self.version.max(3);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                ) => {}
            Err(error) => return Err(error).context("Cannot inspect the managed INI bridge"),
        }
        self.verify_links(game)
    }

    pub(super) fn verify_links(&self, game: &Game) -> Result<()> {
        for link in &self.links {
            link.verify(game)?;
        }
        Ok(())
    }

    pub(super) fn validate_layout(&self, game: &Game) -> Result<()> {
        self.validate_layout_with(game, None)
    }

    pub(super) fn validate_layout_with(
        &self,
        game: &Game,
        created: Option<&BTreeSet<PathBuf>>,
    ) -> Result<()> {
        ensure!(
            self.version >= 3 || (self.directories.is_empty() && self.links.is_empty()),
            "Layout recovery requires its versioned journal format"
        );
        let mut seen = BTreeSet::new();
        for link in &self.links {
            link.validate(game)?;
            ensure!(seen.insert(link.slot), "Duplicate INI bridge");
            ensure!(
                self.changes
                    .iter()
                    .all(|change| change.target != (Target::CustomIni { slot: link.slot })),
                "INI bridge overlaps a writable target"
            );
        }
        let changes = self
            .changes
            .iter()
            .map(|change| {
                change
                    .target
                    .resolve_with(game, created)
                    .map(|path| path_key(&path))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let mut previous = 0;
        let mut seen = BTreeSet::new();
        let mut declared = BTreeSet::new();
        for directory in &self.directories {
            let path = directory.resolve_with(game, created)?;
            let recorded = directory.target.recorded_path(game)?;
            let recorded = recorded
                .ancestors()
                .nth(directory.levels)
                .context("Invalid journal directory")?;
            ensure!(
                declared.insert(recorded.to_owned()),
                "Duplicate recorded journal directory"
            );
            ensure!(
                (seen.insert(path.clone()) || created.is_some())
                    && (!changes.contains(&path_key(&path))
                        || self
                            .changes
                            .iter()
                            .any(|change| change.before == Node::Absent
                                && change.after == (Node::Directory { mode: 0o755 })
                                && change
                                    .target
                                    .resolve_with(game, created)
                                    .is_ok_and(|target| path_key(&target) == path_key(&path)))),
                "Duplicate journal directory"
            );
            ensure!(self.changes.iter().any(|change| change.target == directory.target && change.after != Node::Absent), "Journal directory has no managed output");
            let depth = path.components().count();
            ensure!(
                depth >= previous,
                "Journal directories are not in creation order"
            );
            previous = depth;
        }
        Ok(())
    }

    pub(in crate::core::generations) fn capture_directories(&mut self, game: &Game) -> Result<()> {
        let mut seen = self
            .directories
            .iter()
            .map(|entry| entry.resolve(game).map(|path| path_key(&path)))
            .collect::<Result<BTreeSet<_>>>()?;
        let explicit = self
            .changes
            .iter()
            .map(|change| change.target.resolve(game).map(|path| path_key(&path)))
            .collect::<Result<BTreeSet<_>>>()?;
        for change in &self.changes {
            let path = change.target.resolve(game)?;
            accessible(game, &change.target, &path, true)?;
            if change.after == Node::Absent {
                continue;
            }
            for (levels, parent) in path.ancestors().enumerate().skip(1) {
                if parent == root(game, &change.target)? {
                    break;
                }
                match fs::symlink_metadata(parent) {
                    Ok(metadata) => {
                        ensure!(metadata.is_dir(), "Deployment parent is not a directory");
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        if !explicit.contains(&path_key(parent)) && seen.insert(path_key(parent)) {
                            self.directories.push(Directory {
                                target: change.target.clone(),
                                levels,
                            });
                        }
                    }
                    Err(error) => {
                        return Err(error).context("Cannot prepare deployment directories");
                    }
                }
            }
        }
        let mut keyed = self
            .directories
            .drain(..)
            .map(|entry| Ok((entry.resolve(game)?.components().count(), entry)))
            .collect::<Result<Vec<_>>>()?;
        keyed.sort_by_key(|(depth, _)| *depth);
        self.directories = keyed.into_iter().map(|(_, entry)| entry).collect();
        if !self.directories.is_empty() {
            self.version = self.version.max(3);
        }
        self.validate_layout(game)
    }

    pub(super) fn prepared_parents(&self, game: &Game, target: &Target) -> Result<()> {
        let path = target.resolve(game)?;
        accessible(game, target, &path, true)?;
        let planned = self
            .directories
            .iter()
            .map(|entry| entry.resolve(game).map(|path| path_key(&path)))
            .collect::<Result<BTreeSet<_>>>()?;
        for parent in path.ancestors().skip(1) {
            if parent == root(game, target)? {
                break;
            }
            match fs::symlink_metadata(parent) {
                Ok(metadata) => ensure!(metadata.is_dir(), "Deployment parent is redirected"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ensure!(
                        planned.contains(&path_key(parent))
                            || self.changes.iter().any(|change| change
                                .target
                                .resolve(game)
                                .is_ok_and(|path| path_key(&path) == path_key(parent))
                                && matches!(change.after, Node::Directory { .. })),
                        "Unrecorded deployment parent is missing"
                    );
                }
                Err(error) => return Err(error).context("Cannot verify deployment parent"),
            }
        }
        Ok(())
    }

    pub(super) fn verify_directories(&self, game: &Game, applied: bool) -> Result<()> {
        for directory in &self.directories {
            let path = directory.resolve(game)?;
            accessible(game, &directory.target, &path, !applied)?;
            let expected = if applied {
                Node::Directory { mode: 0o755 }
            } else {
                Node::Absent
            };
            ensure!(
                inspect(&path, &Control::default())? == expected,
                "Prepared deployment directory changed; recovery information was preserved"
            );
        }
        Ok(())
    }

    pub(super) fn recovery_directories(&self, game: &Game) -> Result<BTreeSet<PathBuf>> {
        let mut created = BTreeSet::new();
        for directory in &self.directories {
            let target = directory.target.recorded_path(game)?;
            let path = target
                .ancestors()
                .nth(directory.levels)
                .context("Invalid journal directory")?;
            let root = root(game, &directory.target)?;
            ensure!(
                directory.levels > 0 && path != root && path.starts_with(root),
                "Journal directory escapes its authorized root"
            );
            ensure!(self.changes.iter().any(|change| change.target == directory.target && change.after != Node::Absent), "Journal directory has no managed output");
            created.insert(path.to_owned());
        }
        for change in &self.changes {
            if change.before == Node::Absent && matches!(change.after, Node::Directory { .. }) {
                created.insert(change.target.recorded_path(game)?);
            }
        }
        Ok(created)
    }

    pub(super) fn operations(&self, game: &Game) -> Result<Vec<Operation>> {
        self.operations_with(game, None)
    }

    pub(super) fn operations_with(
        &self,
        game: &Game,
        created: Option<&BTreeSet<PathBuf>>,
    ) -> Result<Vec<Operation>> {
        let mut operations = self
            .changes
            .iter()
            .map(|change| {
                Ok(Operation {
                    target: change.target.clone(),
                    path: change.target.resolve_with(game, created)?,
                    before: change.before.clone(),
                    after: change.after.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        for directory in &self.directories {
            operations.push(Operation {
                target: directory.target.clone(),
                path: directory.resolve_with(game, created)?,
                before: Node::Absent,
                after: Node::Directory { mode: 0o755 },
            });
        }
        if created.is_none() {
            canonicalize_operations(&mut operations)?;
        }
        operations.sort_by_key(|operation| {
            let depth = operation.path.components().count();
            if operation.after == Node::Absent {
                (0, usize::MAX - depth)
            } else {
                (1, depth)
            }
        });
        Ok(operations)
    }
}

pub(super) struct Operation {
    pub(super) target: Target,
    pub(super) path: PathBuf,
    pub(super) before: Node,
    pub(super) after: Node,
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn canonicalize_operations(operations: &mut Vec<Operation>) -> Result<()> {
    let mut paths = operations
        .iter()
        .map(|operation| operation.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    let mut spelling = BTreeMap::new();
    for path in paths {
        let mut prefix = PathBuf::new();
        for part in path.components() {
            let candidate = prefix.join(part);
            match fs::symlink_metadata(&candidate) {
                Ok(_) => prefix = candidate,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let key = (prefix, part.as_os_str().to_string_lossy().to_lowercase());
                    prefix = spelling.entry(key).or_insert(candidate).clone();
                }
                Err(error) => return Err(error).context("Cannot canonicalize deployment layout"),
            }
        }
    }
    for operation in operations.iter_mut() {
        let mut canonical = PathBuf::new();
        for part in operation.path.components() {
            let key = (
                canonical.clone(),
                part.as_os_str().to_string_lossy().to_lowercase(),
            );
            canonical = spelling
                .get(&key)
                .cloned()
                .unwrap_or_else(|| canonical.join(part));
        }
        operation.path = canonical;
    }
    let mut unique = BTreeMap::new();
    for operation in operations.drain(..) {
        let key = path_key(&operation.path);
        if let Some(previous) = unique.get(&key) {
            let previous: &Operation = previous;
            ensure!(
                previous.before == operation.before
                    && previous.after == operation.after
                    && matches!(operation.after, Node::Directory { .. }),
                "Conflicting activation operations at {}",
                operation.path.display()
            );
        } else {
            unique.insert(key, operation);
        }
    }
    operations.extend(unique.into_values());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::generations::catalog::History;

    // @variants: both
    #[tokio::test]
    async fn mixed_case_missing_parents_share_one_directory_and_roll_back() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, _) =
            crate::core::generations::tests::snapshot_fixture(temp.path()).await?;
        fs::create_dir_all(game.data_dir())?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let identity = history
            .retain(temp.path().join("winner/file.txt"), Control::default())
            .await?;
        let file = Node::File {
            identity,
            mode: 0o644,
        };
        let journal = Journal::prepare(
            &history,
            &game,
            vec![
                (Target::file(&game.engine, "Textures/A.bin")?, file.clone()),
                (Target::file(&game.engine, "textures/nested/B.bin")?, file),
                (
                    Target::file(&game.engine, "TEXTURES/Nested/")?,
                    Node::Directory { mode: 0o755 },
                ),
            ],
            Control::default(),
        )
        .await?;
        journal
            .persist(&history, &game, "deploy", BTreeMap::new())
            .await?;
        journal.apply(&history, &game, Control::default()).await?;
        assert_eq!(fs::read_dir(game.data_dir())?.count(), 1);
        assert_eq!(
            fs::read(Target::file(&game.engine, "textures/a.bin")?.resolve(&game)?)?,
            b"winner"
        );
        assert_eq!(
            fs::read(Target::file(&game.engine, "textures/nested/b.bin")?.resolve(&game)?)?,
            b"winner"
        );
        journal.recover(&history, &game, false).await?;
        assert_eq!(fs::read_dir(game.data_dir())?.count(), 0);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn recovers_legacy_case_duplicates_without_discarding_external_edits() -> Result<()> {
        for scenario in 0..4 {
            let edited = scenario == 1;
            let unowned = scenario == 3;
            let temp = tempfile::tempdir()?;
            let (tracker, game, _) =
                crate::core::generations::tests::snapshot_fixture(temp.path()).await?;
            fs::create_dir_all(game.data_dir())?;
            let history = History::open(&tracker, &game.id, temp.path(), true).await?;
            let identity = history
                .retain(temp.path().join("winner/file.txt"), Control::default())
                .await?;
            let first = Target::file(&game.engine, "Textures/A.bin")?;
            let second = Target::file(&game.engine, "textures/B.bin")?;
            let file = Node::File {
                identity,
                mode: 0o644,
            };
            let mut journal = Journal::prepare(
                &history,
                &game,
                vec![(first.clone(), file.clone()), (second.clone(), file)],
                Control::default(),
            )
            .await?;
            journal.directories = vec![
                Directory {
                    target: first,
                    levels: 1,
                },
                Directory {
                    target: second,
                    levels: 1,
                },
            ];
            journal
                .persist(&history, &game, "deploy", BTreeMap::new())
                .await?;
            for name in ["Textures", "textures"] {
                fs::create_dir(game.data_dir().join(name))?;
            }
            fs::write(game.data_dir().join("Textures/A.bin"), b"winner")?;
            fs::write(
                game.data_dir().join("textures/B.bin"),
                if edited { b"edited" } else { b"winner" },
            )?;
            if scenario == 2 {
                fs::remove_file(game.data_dir().join("Textures/A.bin"))?;
                fs::remove_dir(game.data_dir().join("Textures"))?;
            }
            if unowned {
                fs::create_dir(game.data_dir().join("TEXTURES"))?;
            }
            assert_eq!(
                journal.recover(&history, &game, false).await.is_err(),
                edited || unowned
            );
            let pending: i64 =
                sqlx::query_scalar("SELECT count(*) FROM generation_journals WHERE game_id=?")
                    .bind(&game.id)
                    .fetch_one(&tracker.pool)
                    .await?;
            assert_eq!(pending, i64::from(edited || unowned));
            if edited {
                assert_eq!(fs::read(game.data_dir().join("textures/B.bin"))?, b"edited");
            } else if unowned {
                assert!(game.data_dir().join("TEXTURES").is_dir());
                assert_eq!(fs::read(game.data_dir().join("Textures/A.bin"))?, b"winner");
            } else {
                assert_eq!(fs::read_dir(game.data_dir())?.count(), 0);
            }
        }
        Ok(())
    }
}
