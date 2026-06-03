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
) -> Result<(), String> {
    if !is_snap() {
        return Ok(());
    }

    let home = std::env::var_os("HOME").map(PathBuf::from);
    let snap_user_common = user_common_dir();
    let snap_user_data = std::env::var_os("SNAP_USER_DATA").map(PathBuf::from);

    if let Some(message) = classify_snap_path(
        path,
        home.as_deref(),
        snap_user_common.as_deref(),
        snap_user_data.as_deref(),
    ) {
        return Err(message);
    }

    let meta = std::fs::metadata(path).map_err(|e| {
        format!(
            "Cannot access selected {} '{}': {e}",
            kind.label(),
            path.display()
        )
    })?;
    if !meta.is_dir() {
        return Err(format!(
            "Selected {} is not a folder: {}",
            kind.label(),
            path.display()
        ));
    }

    std::fs::read_dir(path).map_err(|e| {
        format!(
            "Cannot read selected {} '{}': {e}",
            kind.label(),
            path.display()
        )
    })?;
    probe_folder_writable(path).map_err(|e| {
        format!(
            "Cannot write to selected {} '{}': {e}",
            kind.label(),
            path.display()
        )
    })?;

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
    {
        return None;
    }

    if is_document_portal_path(path) {
        return Some(
            "The selected folder is a document-portal mount. Choose the original folder path so Deployd can reuse it after restart."
                .to_string(),
        );
    }

    if is_removable_media_path(path) {
        return Some(
            "The selected folder is under /media, /mnt, or /run/media. Deployd's Snap does not currently declare removable-media, so strict confinement blocks that location."
                .to_string(),
        );
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
    use std::path::Path;

    use super::*;

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

    #[test]
    fn rejects_removable_media_without_interface() {
        let path = Path::new("/mnt/games/Skyrim");

        let message = classify_snap_path(path, Some(Path::new("/home/alex")), None, None)
            .expect("removable-media path should be rejected");

        assert!(
            message.contains("removable-media"),
            "message should name the missing interface: {message}"
        );
    }

    #[test]
    fn rejects_document_portal_paths() {
        let path = Path::new("/run/user/1000/doc/abcd/Game");

        let message = classify_snap_path(path, Some(Path::new("/home/alex")), None, None)
            .expect("document portal path should be rejected");

        assert!(
            message.contains("document-portal"),
            "message should explain portal paths are not durable: {message}"
        );
    }
}
