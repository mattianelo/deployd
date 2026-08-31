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

const DEFAULT_SNAP_INSTANCE_NAME: &str = "deployd";

pub(crate) fn removable_media_connect_command() -> String {
    removable_media_connect_command_for(std::env::var_os("SNAP_INSTANCE_NAME").as_deref())
}

fn removable_media_connect_command_for(instance_name: Option<&std::ffi::OsStr>) -> String {
    let instance_name = instance_name
        .and_then(std::ffi::OsStr::to_str)
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_SNAP_INSTANCE_NAME);
    format!("snap connect {instance_name}:removable-media")
}

pub(crate) fn removable_media_connection_message() -> String {
    let command = removable_media_connect_command();
    format!(
        "Deployd needs permission to use this external drive as the downloads folder.\n\n\
         Run this command on your system, then select the folder again:\n\n\
         {command}"
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

pub(crate) fn is_removable_media_path(path: &Path) -> bool {
    path.starts_with("/media") || path.starts_with("/mnt") || path.starts_with("/run/media")
}

pub(crate) fn manual_archive_recovery_message(
    archive_path: &Path,
    downloads_dir: &Path,
    error: &anyhow::Error,
) -> Option<String> {
    let inaccessible = error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            )
        })
    });
    manual_archive_recovery_message_with(is_snap(), archive_path, downloads_dir, inaccessible)
}

fn manual_archive_recovery_message_with(
    is_snap: bool,
    archive_path: &Path,
    downloads_dir: &Path,
    inaccessible: bool,
) -> Option<String> {
    if !is_snap
        || !inaccessible
        || !is_removable_media_path(archive_path)
        || archive_path.starts_with(downloads_dir)
    {
        return None;
    }

    Some(format!(
        "The Snap cannot access this archive's folder on the external drive. Move the archive into the configured downloads folder, '{}', then scan that folder and install it from the Downloads panel.",
        downloads_dir.display()
    ))
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
    fn connection_command_uses_parallel_snap_instance_name() {
        let command =
            removable_media_connect_command_for(Some(std::ffi::OsStr::new("deployd_dev")));

        assert_eq!(command, "snap connect deployd_dev:removable-media");
    }

    // @variants: snap
    #[test]
    fn connection_command_defaults_to_published_snap_name() {
        let command = removable_media_connect_command_for(None);

        assert_eq!(command, "snap connect deployd:removable-media");
    }

    // @variants: snap
    #[test]
    fn explains_how_to_import_an_ungranted_external_archive() {
        let message = manual_archive_recovery_message_with(
            true,
            Path::new("/media/alex/Mods/archive.7z"),
            Path::new("/media/alex/Downloads"),
            true,
        )
        .expect("an inaccessible archive outside the grant needs recovery guidance");

        assert!(message.contains("Move the archive into the configured downloads folder"));
        assert!(message.contains("Downloads panel"));
    }

    // @variants: appimage
    #[test]
    fn leaves_appimage_archive_errors_unchanged() {
        assert_eq!(
            manual_archive_recovery_message_with(
                false,
                Path::new("/media/alex/Mods/archive.7z"),
                Path::new("/media/alex/Downloads"),
                true,
            ),
            None
        );
    }

    // @variants: snap
    #[test]
    fn leaves_granted_download_archive_errors_unchanged() {
        assert_eq!(
            manual_archive_recovery_message_with(
                true,
                Path::new("/media/alex/Downloads/archive.7z"),
                Path::new("/media/alex/Downloads"),
                true,
            ),
            None
        );
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
