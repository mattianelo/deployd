use std::path::PathBuf;

use anyhow::Result;

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
