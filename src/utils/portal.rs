use std::path::{Component, Path, PathBuf};

use anyhow::Result;
use ashpd::documents::{DocumentID, Documents};
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
    match trash_file_by_path(&path).await {
        Ok(()) => Ok(()),
        Err(original_error) => {
            if let Some(host_path) = document_portal_host_path(&path).await {
                return trash_file_by_path(&host_path).await.map_err(|host_error| {
                    anyhow::anyhow!(
                        "document-portal trash failed: {original_error}; host-path trash failed: {host_error}"
                    )
                });
            }

            Err(original_error)
        }
    }
}

async fn trash_file_by_path(path: &Path) -> Result<()> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(open_error) => {
            return trash_file_with_gio(path).map_err(|gio_error| {
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
            trash_file_with_gio(path).map_err(|gio_error| {
                anyhow::anyhow!(
                    "portal trash failed: {portal_error}; filesystem trash failed: {gio_error}"
                )
            })
        }
    }
}

/// Resolve a document-portal route to the corresponding host path when the portal exposes it.
pub(crate) async fn document_portal_host_path(path: &Path) -> Option<PathBuf> {
    let (doc_id, relative_path) = split_document_portal_path(path)?;
    let documents = Documents::new().await.ok()?;
    let host_paths = documents
        .host_paths(std::slice::from_ref(&doc_id))
        .await
        .ok()?;
    let host_path = host_paths.get(&doc_id)?;
    Some(host_path.as_ref().join(relative_path))
}

fn split_document_portal_path(path: &Path) -> Option<(DocumentID, PathBuf)> {
    let mut components = path.components();
    match (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) {
        (
            Some(Component::RootDir),
            Some(Component::Normal(run)),
            Some(Component::Normal(user)),
            Some(Component::Normal(_uid)),
            Some(Component::Normal(doc)),
        ) if run == "run" && user == "user" && doc == "doc" => {}
        _ => return None,
    }

    let first = normal_component(components.next()?)?;
    let doc_id = if first == "by-app" {
        normal_component(components.next()?)?;
        normal_component(components.next()?)?
    } else {
        first
    };
    normal_component(components.next()?)?;
    let relative_path = components.as_path().to_path_buf();
    Some((DocumentID::from(doc_id), relative_path))
}

fn normal_component(component: Component<'_>) -> Option<String> {
    match component {
        Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
        _ => None,
    }
}

fn trash_file_with_gio(path: &Path) -> std::result::Result<(), glib::Error> {
    let gio_file = gio::File::for_path(path);
    gio_file.trash(None::<&gio::Cancellable>)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    // Regression: the document portal inserts the exported entry's basename after its ID.
    // @variants: snap
    #[test]
    fn splits_document_portal_path() {
        let path = Path::new("/run/user/1000/doc/f29584af/Mods/fallout4/Hydra.7z");

        let (doc_id, relative_path) =
            split_document_portal_path(path).expect("document portal path should split");

        assert_eq!(doc_id.as_ref(), "f29584af");
        assert_eq!(relative_path, PathBuf::from("fallout4").join("Hydra.7z"));
    }

    // Regression: sandboxed Snap grants use the document portal's application-scoped domain.
    // @variants: snap
    #[test]
    fn splits_application_scoped_document_portal_path() {
        let path = Path::new(
            "/run/user/1000/doc/by-app/snap.deployd_dev_deployd/56f1c1aa/bcc08294/Gaming/Downloads",
        );

        let (doc_id, relative_path) =
            split_document_portal_path(path).expect("application-scoped portal path should split");

        assert_eq!(doc_id.as_ref(), "56f1c1aa");
        assert_eq!(
            relative_path,
            PathBuf::from("Gaming").join("Downloads")
        );
    }

    // @variants: snap
    #[test]
    fn maps_exported_folder_itself_to_document_host_path() {
        let path = Path::new("/run/user/1000/doc/56f1c1aa/ExternalDrive");

        let (doc_id, relative_path) =
            split_document_portal_path(path).expect("exported folder should split");

        assert_eq!(doc_id.as_ref(), "56f1c1aa");
        assert!(relative_path.as_os_str().is_empty());
    }

    // @variants: both
    #[test]
    fn ignores_non_document_portal_path() {
        let path = Path::new("/home/alex/Mods/fallout4/Hydra.7z");

        assert!(split_document_portal_path(path).is_none());
    }
}
