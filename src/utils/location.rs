use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{portal, snap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelectedLocation {
    pub(crate) root: PathBuf,
    pub(crate) host_hint: Option<PathBuf>,
}

impl SelectedLocation {
    pub(crate) async fn capture(root: PathBuf) -> Self {
        Self::capture_with(root, |path| async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                portal::document_portal_host_path(&path),
            )
            .await
            .ok()
            .flatten()
        })
        .await
    }

    async fn capture_with<F: std::future::Future<Output = Option<PathBuf>>>(
        root: PathBuf,
        resolve: impl FnOnce(PathBuf) -> F,
    ) -> Self {
        let host_hint = if portal::is_document_path(&root) {
            resolve(root.clone()).await
        } else {
            Some(root.clone())
        };
        Self { root, host_hint }
    }

    pub(crate) fn validate_identity(&self, previous: &Self) -> Result<bool> {
        match (&previous.host_hint, &self.host_hint) {
            (Some(old), Some(new)) if old != new => bail!(
                "This is a different location. Use Change folder in Manage Games to change the installation; Restore folder access only reconnects the original folder."
            ),
            (Some(_), Some(_)) => Ok(true),
            _ => Ok(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FolderRole {
    Game,
    Prefix,
}

impl FolderRole {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::Prefix => "prefix",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "game" => Ok(Self::Game),
            "prefix" => Ok(Self::Prefix),
            _ => bail!("Unknown folder role: {value}"),
        }
    }

    pub(crate) fn kind(self) -> snap::SelectedFolderKind {
        match self {
            Self::Game => snap::SelectedFolderKind::GameFolder,
            Self::Prefix => snap::SelectedFolderKind::WinePrefix,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Game => "game folder",
            Self::Prefix => "Wine prefix",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FolderSelection {
    pub(crate) role: FolderRole,
    pub(crate) location: SelectedLocation,
    pub(crate) relative: PathBuf,
}

pub(crate) fn resolve_relative(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("A location binding must stay beneath the selected folder");
    }
    Ok(if relative.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FolderChange {
    pub(crate) game_id: String,
    pub(crate) role: FolderRole,
    pub(crate) old_path: PathBuf,
    pub(crate) new_path: PathBuf,
}

pub(crate) fn rebase(path: &Path, old: &Path, new: &Path) -> Result<Option<PathBuf>> {
    let Ok(relative) = path.strip_prefix(old) else {
        return Ok(None);
    };
    Ok(Some(resolve_relative(new, relative)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: snap
    #[tokio::test]
    async fn retains_the_selected_root_when_host_lookup_is_unavailable() {
        let root = PathBuf::from("/run/user/1000/doc/id/Game/ME1");
        let location = SelectedLocation::capture_with(root.clone(), |_| async { None }).await;
        assert_eq!(location.root, root);
        assert!(location.host_hint.is_none());
    }

    // @variants: appimage
    #[tokio::test]
    async fn does_not_query_the_portal_for_a_direct_location() {
        let root = PathBuf::from("/games/direct");
        let location = SelectedLocation::capture_with(root.clone(), |_| async {
            panic!("direct paths do not need portal lookup")
        })
        .await;
        assert_eq!(location.host_hint, Some(root));
    }

    // @variants: both
    #[test]
    fn rejects_bindings_outside_the_selected_root() {
        for relative in ["../Game", "/Game", "Game/../other"] {
            assert!(resolve_relative(Path::new("/selected"), Path::new(relative)).is_err());
        }
        assert_eq!(
            resolve_relative(Path::new("/selected"), Path::new("Game/ME1")).unwrap(),
            Path::new("/selected/Game/ME1")
        );
    }

    // @variants: snap
    #[test]
    fn rejects_a_different_known_host_location() {
        let old = SelectedLocation {
            root: "/doc/old".into(),
            host_hint: Some("/games/original".into()),
        };
        let new = SelectedLocation {
            root: "/doc/new".into(),
            host_hint: Some("/games/other".into()),
        };
        assert!(new.validate_identity(&old).is_err());
        assert!(
            !SelectedLocation {
                host_hint: None,
                ..new
            }
            .validate_identity(&old)
            .unwrap()
        );
    }

    // @variants: snap
    #[test]
    fn rebases_components_without_matching_sibling_names() {
        assert_eq!(
            rebase(
                Path::new("/doc/old/Game/file"),
                Path::new("/doc/old/Game"),
                Path::new("/doc/new/Game")
            )
            .unwrap(),
            Some("/doc/new/Game/file".into())
        );
        assert!(
            rebase(
                Path::new("/doc/old/Game2/file"),
                Path::new("/doc/old/Game"),
                Path::new("/doc/new/Game")
            )
            .unwrap()
            .is_none()
        );
    }
}
