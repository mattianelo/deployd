use std::path::{Path, PathBuf};

use anyhow::Result;
use gio::prelude::*;

/// Open a folder-picker dialog via the xdg-desktop-portal FileChooser portal.
///
/// Returns `Ok(Some(path))` when the user confirms, `Ok(None)` when the URI
/// list is empty, and `Err` on cancellation or portal unavailability.
/// Callers that use `if let Ok(Some(path))` treat all outcomes correctly.
pub async fn select_folder(title: &str) -> Result<Option<PathBuf>> {
    let files = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title)
        .directory(true)
        .send()
        .await?
        .response()?;

    let path = files
        .uris()
        .first()
        .and_then(|u| glib::filename_from_uri(u.as_str()).ok())
        .map(|(p, _)| p);

    Ok(path)
}

/// Move a file to the desktop Trash.
///
/// The portal path is preferred because it works with strict Snap confinement
/// without requiring Deployd to write directly into hidden home directories.
pub async fn trash_file(path: PathBuf) -> Result<()> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(open_error) => {
            return trash_file_with_gio(&path).map_err(|gio_error| {
                anyhow::anyhow!(
                    "could not open file for portal trash: {open_error}; filesystem trash failed: {gio_error}"
                )
            });
        }
    };

    match ashpd::desktop::trash::trash_file(&file).await {
        Ok(()) => Ok(()),
        Err(portal_error) => {
            drop(file);
            trash_file_with_gio(&path).map_err(|gio_error| {
                anyhow::anyhow!(
                    "portal trash failed: {portal_error}; filesystem trash failed: {gio_error}"
                )
            })
        }
    }
}

fn trash_file_with_gio(path: &Path) -> std::result::Result<(), glib::Error> {
    let gio_file = gio::File::for_path(path);
    gio_file.trash(None::<&gio::Cancellable>)
}
