use std::fs::OpenOptions;
use std::path::{Component, Path, PathBuf};

/// A user-selected folder whose suitability depends on Snap confinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectedFolderKind {
    GameFolder,
    WinePrefix,
    CacheFolder,
    DownloadsFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectedFolderRecovery {
    ConnectRemovableMedia,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SelectedFolderError {
    message: String,
    recovery: Option<SelectedFolderRecovery>,
}

impl SelectedFolderError {
    pub(crate) fn recovery(&self) -> Option<SelectedFolderRecovery> {
        self.recovery
    }
}

impl std::fmt::Display for SelectedFolderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub(crate) const REMOVABLE_MEDIA_CONNECT_COMMAND: &str = "snap connect deployd:removable-media";

pub(crate) fn removable_media_connection_message() -> String {
    format!(
        "Deployd needs permission to use this external drive as the downloads folder.\n\n\
         Run this command on your system, then select the folder again:\n\n\
         {REMOVABLE_MEDIA_CONNECT_COMMAND}"
    )
}

impl SelectedFolderKind {
    fn label(self) -> &'static str {
        match self {
            Self::GameFolder => "game folder",
            Self::WinePrefix => "Wine prefix folder",
            Self::CacheFolder => "cache folder",
            Self::DownloadsFolder => "downloads folder",
        }
    }
}

/// True when the process is running inside Deployd's Snap package.
pub(crate) fn is_snap() -> bool {
    std::env::var_os("SNAP").is_some()
}

/// Durable per-user Snap data root, if this process is running inside a Snap.
pub(crate) fn user_common_dir() -> Option<PathBuf> {
    std::env::var_os("SNAP_USER_COMMON").map(PathBuf::from)
}

/// Validate a folder selected by the user before persisting it in Snap state.
///
/// Under AppImage/source builds this is intentionally a no-op so existing unrestricted
/// behaviour stays unchanged. Under Snap it rejects paths known to be outside strict
/// confinement and probes the selected folder for read/write access.
pub(crate) fn validate_selected_folder(
    path: &Path,
    kind: SelectedFolderKind,
) -> Result<(), SelectedFolderError> {
    let environment = SnapEnvironment::from_process();
    validate_selected_folder_with(path, kind, environment.as_ref(), inspect_selected_folder)
}

#[derive(Debug)]
struct SnapEnvironment {
    home: Option<PathBuf>,
    user_common: Option<PathBuf>,
    user_data: Option<PathBuf>,
}

impl SnapEnvironment {
    fn from_process() -> Option<Self> {
        is_snap().then(|| Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
            user_common: user_common_dir(),
            user_data: std::env::var_os("SNAP_USER_DATA").map(PathBuf::from),
        })
    }
}

enum FolderAccessError {
    Metadata(std::io::Error),
    NotDirectory,
    Read(std::io::Error),
    Write(std::io::Error),
}

fn validate_selected_folder_with(
    path: &Path,
    kind: SelectedFolderKind,
    environment: Option<&SnapEnvironment>,
    inspect: impl FnOnce(&Path) -> Result<(), FolderAccessError>,
) -> Result<(), SelectedFolderError> {
    let Some(environment) = environment else {
        return Ok(());
    };

    if let Some(message) = classify_snap_path(
        path,
        environment.home.as_deref(),
        environment.user_common.as_deref(),
        environment.user_data.as_deref(),
    ) {
        return Err(SelectedFolderError {
            message,
            recovery: None,
        });
    }

    inspect(path).map_err(|error| SelectedFolderError {
        recovery: removable_media_recovery(path, &error),
        message: match error {
            FolderAccessError::Metadata(error) => format!(
                "Cannot access selected {} '{}': {e}",
                kind.label(),
                path.display(),
                e = error
            ),
            FolderAccessError::NotDirectory => format!(
                "Selected {} is not a folder: {}",
                kind.label(),
                path.display()
            ),
            FolderAccessError::Read(error) => format!(
                "Cannot read selected {} '{}': {e}",
                kind.label(),
                path.display(),
                e = error
            ),
            FolderAccessError::Write(error) => format!(
                "Cannot write to selected {} '{}': {e}",
                kind.label(),
                path.display(),
                e = error
            ),
        },
    })
}

fn removable_media_recovery(
    path: &Path,
    error: &FolderAccessError,
) -> Option<SelectedFolderRecovery> {
    if !is_removable_media_path(path) {
        return None;
    }

    let permission_denied = match error {
        FolderAccessError::Metadata(error) | FolderAccessError::Read(error) => {
            error.kind() == std::io::ErrorKind::PermissionDenied
        }
        FolderAccessError::NotDirectory | FolderAccessError::Write(_) => false,
    };

    permission_denied.then_some(SelectedFolderRecovery::ConnectRemovableMedia)
}

fn inspect_selected_folder(path: &Path) -> Result<(), FolderAccessError> {
    let meta = std::fs::metadata(path).map_err(FolderAccessError::Metadata)?;
    if !meta.is_dir() {
        return Err(FolderAccessError::NotDirectory);
    }

    std::fs::read_dir(path).map_err(FolderAccessError::Read)?;
    probe_folder_writable(path).map_err(FolderAccessError::Write)?;

    Ok(())
}

fn classify_snap_path(
    path: &Path,
    home: Option<&Path>,
    snap_user_common: Option<&Path>,
    snap_user_data: Option<&Path>,
) -> Option<String> {
    if snap_user_common.is_some_and(|root| path.starts_with(root))
        || snap_user_data.is_some_and(|root| path.starts_with(root))
        || is_document_portal_path(path)
    {
        return None;
    }

    if let Some(home) = home
        && let Ok(rel) = path.strip_prefix(home)
        && first_component_is_hidden(rel)
    {
        return Some(
            "The selected folder is inside a hidden home directory. Strict Snaps cannot access hidden home paths unless a reviewed personal-files interface is declared."
                .to_string(),
        );
    }

    None
}

fn first_component_is_hidden(path: &Path) -> bool {
    path.components().next().is_some_and(|component| {
        if let Component::Normal(name) = component {
            let name = name.to_string_lossy();
            name.starts_with('.') && name != "."
        } else {
            false
        }
    })
}

fn is_removable_media_path(path: &Path) -> bool {
    path.starts_with("/media") || path.starts_with("/mnt") || path.starts_with("/run/media")
}

fn is_document_portal_path(path: &Path) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();

    parts.len() >= 4 && parts[0] == "run" && parts[1] == "user" && parts[3] == "doc"
}

fn probe_folder_writable(path: &Path) -> std::io::Result<()> {
    let probe_name = format!(
        ".deployd-access-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    let probe_path = path.join(probe_name);
    let _file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)?;
    // Best-effort cleanup: validation has already succeeded if removal fails.
    let _ = std::fs::remove_file(&probe_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::Path;

    use super::*;

    // @variants: snap
    #[test]
    fn accepts_snap_user_common_path() {
        let path = Path::new("/home/alex/snap/deployd/common/deployd/cache");

        assert_eq!(
            classify_snap_path(
                path,
                Some(Path::new("/home/alex")),
                Some(Path::new("/home/alex/snap/deployd/common")),
                None,
            ),
            None
        );
    }

    // @variants: snap
    #[test]
    fn rejects_hidden_home_path_under_snap() {
        let path = Path::new("/home/alex/.steam/steamapps/common/Skyrim");

        let message = classify_snap_path(path, Some(Path::new("/home/alex")), None, None)
            .expect("hidden home path should be rejected");

        assert!(
            message.contains("hidden home directory"),
            "message should explain the Snap hidden-home restriction: {message}"
        );
    }

    // @variants: snap
    #[test]
    fn accepts_removable_media_when_access_probe_succeeds() {
        let environment = SnapEnvironment {
            home: Some(PathBuf::from("/home/alex")),
            user_common: None,
            user_data: None,
        };
        let path = Path::new("/mnt/games/Skyrim");

        let result = validate_selected_folder_with(
            path,
            SelectedFolderKind::GameFolder,
            Some(&environment),
            |_| Ok(()),
        );

        assert_eq!(result, Ok(()));
    }

    // Regression: portal-selected folders remain accessible across sessions and must not be
    // mistaken for ungranted paths outside Snap confinement.
    // @variants: snap
    #[test]
    fn accepts_document_portal_paths() {
        let path = Path::new("/run/user/1000/doc/abcd/Game");

        assert_eq!(
            classify_snap_path(path, Some(Path::new("/home/alex")), None, None),
            None
        );
    }

    // @variants: snap
    #[test]
    fn accepts_portal_granted_downloads_folder() {
        let environment = SnapEnvironment {
            home: Some(PathBuf::from("/home/alex")),
            user_common: None,
            user_data: None,
        };
        let path = Path::new("/run/user/1000/doc/abcd/Downloads");

        let result = validate_selected_folder_with(
            path,
            SelectedFolderKind::DownloadsFolder,
            Some(&environment),
            |_| Ok(()),
        );

        assert_eq!(result, Ok(()));
    }

    // @variants: snap
    #[test]
    fn rejects_ungranted_removable_downloads_folder() {
        let environment = SnapEnvironment {
            home: Some(PathBuf::from("/home/alex")),
            user_common: None,
            user_data: None,
        };
        let path = Path::new("/media/alex/External/Downloads");

        let error = validate_selected_folder_with(
            path,
            SelectedFolderKind::DownloadsFolder,
            Some(&environment),
            |_| {
                Err(FolderAccessError::Metadata(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "confined",
                )))
            },
        )
        .expect_err("ungranted removable-media path should be rejected");

        assert_eq!(
            error.recovery(),
            Some(SelectedFolderRecovery::ConnectRemovableMedia)
        );
        assert!(
            error
                .to_string()
                .contains("Cannot access selected downloads folder")
        );
    }

    // @variants: snap
    #[test]
    fn does_not_suggest_connection_for_read_only_removable_media() {
        let environment = SnapEnvironment {
            home: Some(PathBuf::from("/home/alex")),
            user_common: None,
            user_data: None,
        };
        let path = Path::new("/media/alex/ReadOnly/Downloads");

        let error = validate_selected_folder_with(
            path,
            SelectedFolderKind::DownloadsFolder,
            Some(&environment),
            |_| {
                Err(FolderAccessError::Write(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "read-only filesystem",
                )))
            },
        )
        .expect_err("read-only removable media should be rejected");

        assert_eq!(error.recovery(), None);
        assert!(
            error
                .to_string()
                .contains("Cannot write to selected downloads folder")
        );
    }

    // @variants: snap
    #[test]
    fn removable_media_message_includes_manual_connection_command() {
        let message = removable_media_connection_message();

        assert!(message.contains(REMOVABLE_MEDIA_CONNECT_COMMAND));
        assert!(message.contains("select the folder again"));
    }

    // @variants: appimage
    #[test]
    fn appimage_validation_does_not_probe_the_filesystem() {
        let result = validate_selected_folder_with(
            Path::new("/unrestricted/appimage/path"),
            SelectedFolderKind::GameFolder,
            None,
            |_| panic!("AppImage validation must remain a no-op"),
        );

        assert_eq!(result, Ok(()));
    }

    // @variants: snap
    #[test]
    fn snap_validation_reports_the_failed_filesystem_operation() {
        let environment = SnapEnvironment {
            home: Some(PathBuf::from("/home/alex")),
            user_common: Some(PathBuf::from("/home/alex/snap/deployd/common")),
            user_data: None,
        };
        let path = Path::new("/home/alex/Games/Skyrim");

        let error = validate_selected_folder_with(
            path,
            SelectedFolderKind::GameFolder,
            Some(&environment),
            |_| {
                Err(FolderAccessError::Write(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "confined",
                )))
            },
        )
        .expect_err("failed write probe must reject the selected folder");

        assert!(
            error
                .to_string()
                .contains("Cannot write to selected game folder")
        );
        assert!(error.to_string().contains("confined"));
    }

    // @variants: snap
    #[test]
    fn snap_validation_accepts_portal_grant_after_access_probe() {
        let environment = SnapEnvironment {
            home: Some(PathBuf::from("/home/alex")),
            user_common: None,
            user_data: None,
        };
        let path = Path::new("/run/user/1000/doc/abcd/Game");
        let mut probed = false;

        let result = validate_selected_folder_with(
            path,
            SelectedFolderKind::GameFolder,
            Some(&environment),
            |_| {
                probed = true;
                Ok(())
            },
        );

        assert_eq!(result, Ok(()));
        assert!(
            probed,
            "portal grants must still be checked for live access"
        );
    }
}
