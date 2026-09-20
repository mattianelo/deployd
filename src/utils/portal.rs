use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use ashpd::documents::{DocumentID, Documents};
use gio::prelude::*;

/// Open a folder-picker dialog via the xdg-desktop-portal FileChooser portal.
///
/// Returns `Ok(Some(path))` when the user confirms, `Ok(None)` when the URI
/// list is empty, and `Err` on cancellation or portal unavailability.
/// Callers that use `if let Ok(Some(path))` treat all outcomes correctly.
pub async fn select_folder(title: &str) -> Result<Option<PathBuf>> {
    select_folder_at(title, None).await
}

pub(crate) async fn select_folder_at(
    title: &str,
    initial: Option<&Path>,
) -> Result<Option<PathBuf>> {
    let files = ashpd::desktop::file_chooser::SelectedFiles::open_file()
        .title(title)
        .directory(true)
        .current_folder::<&Path>(initial)?
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

pub(crate) async fn select_location(
    title: &str,
    initial: Option<&Path>,
    kind: crate::utils::snap::SelectedFolderKind,
) -> Result<Option<crate::utils::location::SelectedLocation>> {
    let Some(path) = select_folder_path(title, initial).await? else {
        return Ok(None);
    };
    let location = crate::utils::location::SelectedLocation::capture(path).await;
    validate_location(&location, kind).await?;
    Ok(Some(location))
}

pub(crate) async fn select_prefix_recovery_location(
    title: &str,
    previous: &crate::utils::location::SelectedLocation,
) -> Result<Option<crate::utils::location::SelectedLocation>> {
    let (initial, child) = prefix_recovery_parent(previous)?;
    let Some(path) = select_folder_path(title, initial.as_deref()).await? else {
        return Ok(None);
    };
    let parent = crate::utils::location::SelectedLocation::capture(path).await;
    let location = append_selected_child(parent, &child);
    validate_location(
        &location,
        crate::utils::snap::SelectedFolderKind::WinePrefix,
    )
    .await
    .with_context(|| {
        format!(
            "The selected folder does not contain the saved Wine prefix '{}'",
            child.to_string_lossy()
        )
    })?;
    Ok(Some(location))
}

async fn select_folder_path(title: &str, initial: Option<&Path>) -> Result<Option<PathBuf>> {
    match select_folder_at(title, initial).await {
        Err(error)
            if matches!(
                error.downcast_ref::<ashpd::Error>(),
                Some(ashpd::Error::Response(
                    ashpd::desktop::ResponseError::Cancelled
                ))
            ) =>
        {
            Ok(None)
        }
        result => result,
    }
}

async fn validate_location(
    location: &crate::utils::location::SelectedLocation,
    kind: crate::utils::snap::SelectedFolderKind,
) -> Result<()> {
    let root = location.root.clone();
    tokio::task::spawn_blocking(move || {
        crate::utils::snap::validate_selected_folder(&root, kind)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    })
    .await??;
    Ok(())
}

fn prefix_recovery_parent(
    previous: &crate::utils::location::SelectedLocation,
) -> Result<(Option<PathBuf>, OsString)> {
    let known_path = previous.host_hint.as_deref().unwrap_or(&previous.root);
    let child = known_path
        .file_name()
        .context("The saved Wine prefix has no folder name")?
        .to_owned();
    let initial = previous
        .host_hint
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    Ok((initial, child))
}

fn append_selected_child(
    parent: crate::utils::location::SelectedLocation,
    child: &OsStr,
) -> crate::utils::location::SelectedLocation {
    crate::utils::location::SelectedLocation {
        root: parent.root.join(child),
        host_hint: parent.host_hint.map(|path| path.join(child)),
    }
}

pub(crate) fn is_document_path(path: &Path) -> bool {
    split_document_portal_path(path).is_some()
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

pub(crate) async fn delete_file_permanently(path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(
            "could not permanently delete {}: {error}",
            path.display()
        )),
    })
    .await
    .map_err(|error| anyhow::anyhow!("permanent-delete task failed: {error}"))?
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

    // @variants: both
    #[tokio::test]
    async fn permanent_delete_removes_only_the_requested_file() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let target = directory.path().join("delete.zip");
        let sibling = directory.path().join("keep.zip");
        std::fs::write(&target, "delete").expect("write target");
        std::fs::write(&sibling, "keep").expect("write sibling");

        delete_file_permanently(target.clone())
            .await
            .expect("delete target");

        assert!(!target.exists());
        assert!(sibling.exists());
    }

    // @variants: both
    #[tokio::test]
    async fn permanent_delete_accepts_an_already_missing_file() {
        let directory = tempfile::tempdir().expect("create temp directory");

        delete_file_permanently(directory.path().join("missing.zip"))
            .await
            .expect("accept missing target");
    }

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
        assert_eq!(relative_path, PathBuf::from("Gaming").join("Downloads"));
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

    // A Proton refresh replaces pfx, so recovery grants its stable parent and retains pfx as a
    // child of that grant.
    // @variants: snap
    #[test]
    fn derives_prefix_from_a_stable_parent_grant() {
        let previous = crate::utils::location::SelectedLocation {
            root: "/run/user/1000/doc/old/pfx".into(),
            host_hint: Some("/home/alex/compatdata/1328670/pfx".into()),
        };

        let (initial, child) = prefix_recovery_parent(&previous).expect("derive parent");
        assert_eq!(initial, Some("/home/alex/compatdata/1328670".into()));
        assert_eq!(child, "pfx");

        let selected = append_selected_child(
            crate::utils::location::SelectedLocation {
                root: "/run/user/1000/doc/new/1328670".into(),
                host_hint: Some("/home/alex/compatdata/1328670".into()),
            },
            &child,
        );
        assert_eq!(
            selected.root,
            Path::new("/run/user/1000/doc/new/1328670/pfx")
        );
        assert_eq!(
            selected.host_hint.as_deref(),
            Some(Path::new("/home/alex/compatdata/1328670/pfx"))
        );
    }

    // Older records can lack a host hint. Recovery still knows which child to retain, but leaves
    // the picker to choose its own starting directory.
    // @variants: snap
    #[test]
    fn recovers_prefix_name_without_a_host_hint() {
        let previous = crate::utils::location::SelectedLocation {
            root: "/run/user/1000/doc/old/custom-prefix".into(),
            host_hint: None,
        };

        let (initial, child) = prefix_recovery_parent(&previous).expect("derive child");
        assert_eq!(initial, None);
        assert_eq!(child, "custom-prefix");
    }
}
