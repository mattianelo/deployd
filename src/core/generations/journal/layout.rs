use std::collections::BTreeSet;
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
        let target = self.target.resolve(game)?;
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
            .map(|change| change.target.resolve(game))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut previous = 0;
        let mut seen = BTreeSet::new();
        for directory in &self.directories {
            let path = directory.resolve(game)?;
            ensure!(
                seen.insert(path.clone()) && !changes.contains(&path),
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
            .map(|entry| entry.resolve(game))
            .collect::<Result<BTreeSet<_>>>()?;
        let explicit = self
            .changes
            .iter()
            .map(|change| change.target.resolve(game))
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
                        if !explicit.contains(parent) && seen.insert(parent.to_owned()) {
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
            .map(|entry| entry.resolve(game))
            .collect::<Result<BTreeSet<_>>>()?;
        for parent in path.ancestors().skip(1) {
            if parent == root(game, target)? {
                break;
            }
            match fs::symlink_metadata(parent) {
                Ok(metadata) => ensure!(metadata.is_dir(), "Deployment parent is redirected"),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ensure!(
                        planned.contains(parent)
                            || self.changes.iter().any(|change| change
                                .target
                                .resolve(game)
                                .ok()
                                .as_deref()
                                == Some(parent)
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

    pub(super) fn operations(&self, game: &Game) -> Result<Vec<Operation>> {
        let mut operations = self
            .changes
            .iter()
            .map(|change| {
                Ok(Operation {
                    target: change.target.clone(),
                    path: change.target.resolve(game)?,
                    before: change.before.clone(),
                    after: change.after.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        for directory in &self.directories {
            operations.push(Operation {
                target: directory.target.clone(),
                path: directory.resolve(game)?,
                before: Node::Absent,
                after: Node::Directory { mode: 0o755 },
            });
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
