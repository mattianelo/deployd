use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, bail};
use tokio::sync::RwLock;

use crate::models::game::Game;
use crate::utils::location::{FolderChange, FolderRole, SelectedLocation, resolve_relative};
use crate::utils::{paths, snap};

use super::tracker::locations::{LocationRecord, PendingRecovery};
use super::tracker::{PersistedGame, Tracker};

pub(crate) fn activity_lock() -> Arc<RwLock<()>> {
    static LOCK: OnceLock<Arc<RwLock<()>>> = OnceLock::new();
    LOCK.get_or_init(|| Arc::new(RwLock::new(()))).clone()
}

pub(crate) fn persisted_game(record: PersistedGame) -> Game {
    Game {
        id: record.id,
        title: record.title,
        path: record.path,
        data_subdir: record.data_subdir,
        wine_prefix: record.wine_prefix,
        engine: record.engine,
    }
}

pub(crate) async fn validate_selection(
    record: LocationRecord,
    selected: SelectedLocation,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        for binding in &record.bindings {
            let path = resolve_relative(&selected.root, &binding.relative)?;
            validate_bound_folder(&selected.root, &path, binding.role).with_context(|| {
                format!(
                    "Cannot restore {} for {}",
                    binding.role.label(),
                    binding.title
                )
            })?;
        }
        Ok(())
    })
    .await
    .context("Folder validation task failed")?
}

fn validate_bound_folder(root: &Path, path: &Path, role: FolderRole) -> Result<()> {
    let canonical_root =
        std::fs::canonicalize(root).context("The selected folder is unavailable")?;
    let canonical_path =
        std::fs::canonicalize(path).context("A configured subfolder is unavailable")?;
    if !canonical_path.starts_with(canonical_root) {
        bail!("A configured subfolder escapes the selected folder through a symbolic link")
    }
    snap::validate_selected_folder(path, role.kind())
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if !path.is_dir() {
        bail!("The selected location is not a folder")
    }
    if role == FolderRole::Prefix && !path.join("drive_c").is_dir() {
        bail!("Select the original Wine prefix containing drive_c")
    }
    Ok(())
}

pub(crate) async fn resume_repairs(tracker: &Tracker) -> Result<()> {
    for pending in tracker.pending_location_repairs().await? {
        repair(tracker, &pending).await?;
    }
    Ok(())
}

pub(crate) async fn blocked_games(tracker: &Tracker) -> Result<Vec<String>> {
    if !snap::is_snap() {
        return Ok(Vec::new());
    }
    let pending: HashSet<_> = tracker
        .pending_location_repairs()
        .await?
        .into_iter()
        .flat_map(|repair| repair.changes.into_iter().map(|change| change.game_id))
        .collect();
    let games = tracker.load_persisted_games().await?;
    tokio::task::spawn_blocking(move || {
        games
            .into_iter()
            .filter(|game| {
                pending.contains(&game.id)
                    || snap::validate_selected_folder(
                        &game.path,
                        snap::SelectedFolderKind::GameFolder,
                    )
                    .is_err()
                    || game.wine_prefix.as_deref().is_some_and(|prefix| {
                        snap::validate_selected_folder(prefix, snap::SelectedFolderKind::WinePrefix)
                            .is_err()
                    })
            })
            .map(|game| game.id)
            .collect()
    })
    .await
    .context("Folder access inspection failed")
}

pub(crate) async fn repair(tracker: &Tracker, pending: &PendingRecovery) -> Result<()> {
    if !snap::is_snap() {
        bail!("Folder access repair must be completed in the Snap instance that owns the grants")
    }
    let games = tracker.load_games(true).await?;
    let data_root = paths::deployd_data_dir()?;
    let ids: HashSet<_> = pending
        .changes
        .iter()
        .map(|change| change.game_id.as_str())
        .collect();
    for id in ids {
        let record = games
            .iter()
            .find(|game| game.id == id)
            .context("A game needed for folder repair is no longer managed")?;
        let game = persisted_game(record.clone());
        let changes: Vec<_> = pending
            .changes
            .iter()
            .filter(|change| change.game_id == id)
            .cloned()
            .collect();
        for change in &changes {
            let current = match change.role {
                FolderRole::Game => Some(game.path.as_path()),
                FolderRole::Prefix => game.wine_prefix.as_deref(),
            };
            if current != Some(change.new_path.as_path()) {
                bail!("The repair record no longer matches the game's configured folders")
            }
        }
        let root = data_root.clone();
        let repaired_game = game.clone();
        let repaired_changes = changes.clone();
        tokio::task::spawn_blocking(move || {
            for change in &repaired_changes {
                validate_bound_folder(&change.new_path, &change.new_path, change.role)?;
            }
            repair_links(&root, &repaired_game, &repaired_changes)
        })
        .await
        .context("Folder link repair task failed")??;
        if super::save_manager::rebase_location_journal(&game, &changes).await? {
            let profile = tracker.get_active_profile(&game.id).await?.context(
                "The interrupted save operation has no active profile; it was preserved",
            )?;
            let active = super::save_manager::SaveSetId::for_profile(
                &game.id,
                &profile.id,
                &profile.save_mode,
            );
            super::save_manager::recover_interrupted_transition(&game, &active).await?;
        }
    }
    tracker.finish_location_repair(pending).await
}

fn repair_links(data_root: &Path, game: &Game, changes: &[FolderChange]) -> Result<()> {
    let tool_prefix = paths::snap_tool_prefix_in(data_root, &game.id)?;
    let tool_metadata = match tool_prefix.symlink_metadata() {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Cannot inspect the Snap tool prefix"),
    };
    if tool_metadata.is_some() {
        require_contained(data_root, &tool_prefix)?;
        if tool_prefix.is_symlink() {
            bail!("The Snap tool prefix was replaced by a symbolic link; it was preserved")
        }
        super::game::repair_registered_game_path(game, &tool_prefix, changes)?;
        let prefixes: Vec<_> = changes
            .iter()
            .filter(|change| change.role == FolderRole::Prefix)
            .collect();
        if let Some(first) = prefixes.first() {
            let old: Vec<_> = prefixes
                .iter()
                .map(|change| change.old_path.as_path())
                .collect();
            super::tool_launcher::repair_source_bridges(&tool_prefix, &old, &first.new_path)?;
        }
        let devices = tool_prefix.join("dosdevices");
        if devices.is_dir() {
            require_contained(&tool_prefix, &devices)?;
            for entry in std::fs::read_dir(devices)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.len() != 2
                    || !name.ends_with(':')
                    || matches!(name.as_ref(), "c:" | "z:" | "m:")
                {
                    continue;
                }
                let path = entry.path();
                let Ok(target) = std::fs::read_link(&path) else {
                    continue;
                };
                for change in changes {
                    if let Some(new) =
                        crate::utils::location::rebase(&target, &change.old_path, &change.new_path)?
                    {
                        if target != change.old_path || change.role != FolderRole::Game {
                            bail!(
                                "The Wine drive mapping '{}' has an unrecognized target; it was preserved",
                                path.display()
                            );
                        }
                        replace_owned_link(&path, &target, &new)?;
                        break;
                    }
                }
            }
        }
    }
    for change in changes {
        if change.role == FolderRole::Prefix {
            super::game::repair_ini_links(game, &change.old_path, &change.new_path)?;
        }
    }
    Ok(())
}

pub(crate) fn require_contained(root: &Path, path: &Path) -> Result<()> {
    if !std::fs::canonicalize(path)?.starts_with(std::fs::canonicalize(root)?) {
        bail!("A repair path escapes its owning folder; it was preserved");
    }
    Ok(())
}

pub(crate) fn replace_owned_link(link: &Path, old: &Path, new: &Path) -> Result<()> {
    let actual = std::fs::read_link(link).with_context(|| {
        format!(
            "The generated link '{}' was changed or removed; it was preserved",
            link.display()
        )
    })?;
    if actual == new {
        return Ok(());
    }
    if actual != old {
        bail!(
            "The generated link '{}' was modified; it was preserved",
            link.display()
        )
    }
    let parent = link.parent().context("A generated link has no parent")?;
    let temporary = parent.join(format!(".deployd-link-repair-{}", uuid::Uuid::new_v4()));
    std::os::unix::fs::symlink(new, &temporary)?;
    let result = (|| {
        if std::fs::read_link(link)? != old {
            bail!("The generated link changed during repair")
        }
        std::fs::rename(&temporary, link).context("Could not publish the repaired link")
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::game::GameEngine;

    fn bridge_fixture() -> Result<(
        tempfile::TempDir,
        Game,
        Vec<FolderChange>,
        std::path::PathBuf,
    )> {
        let temp = tempfile::tempdir()?;
        let game = Game {
            id: "fixture".to_string(),
            title: "Fixture".to_string(),
            path: temp.path().join("game"),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: Some(temp.path().join("new-prefix")),
        };
        let source = temp.path().join("new-prefix/drive_c/users/steamuser");
        std::fs::create_dir_all(source.join("Documents"))?;
        std::fs::create_dir_all(source.join("AppData"))?;
        let tool = paths::snap_tool_prefix_in(temp.path(), "fixture")?;
        let user = tool.join("drive_c/users/steamuser");
        std::fs::create_dir_all(&user)?;
        for relative in ["Documents", "AppData"] {
            std::os::unix::fs::symlink(
                temp.path()
                    .join("old-prefix/drive_c/users/steamuser")
                    .join(relative),
                user.join(relative),
            )?;
        }
        let changes = vec![FolderChange {
            game_id: "fixture".to_string(),
            role: FolderRole::Prefix,
            old_path: temp.path().join("old-prefix"),
            new_path: temp.path().join("new-prefix"),
        }];
        Ok((temp, game, changes, user))
    }

    // @variants: snap
    #[test]
    fn repairs_tool_bridges_after_the_old_grant_disappears() -> Result<()> {
        let (temp, game, changes, user) = bridge_fixture()?;
        for _ in 0..2 {
            repair_links(temp.path(), &game, &changes)?;
        }
        for relative in ["Documents", "AppData"] {
            assert_eq!(
                std::fs::read_link(user.join(relative))?,
                temp.path()
                    .join("new-prefix/drive_c/users/steamuser")
                    .join(relative)
            );
        }
        Ok(())
    }

    // @variants: snap
    #[test]
    fn preserves_a_user_modified_tool_bridge() -> Result<()> {
        let (temp, game, changes, user) = bridge_fixture()?;
        let link = user.join("Documents");
        std::fs::remove_file(&link)?;
        std::os::unix::fs::symlink("/user-chosen/Documents", &link)?;
        assert!(repair_links(temp.path(), &game, &changes).is_err());
        assert_eq!(
            std::fs::read_link(link)?,
            Path::new("/user-chosen/Documents")
        );
        Ok(())
    }

    // @variants: snap
    #[test]
    fn resumes_partially_repaired_links_after_another_grant_change() -> Result<()> {
        let (temp, game, mut changes, user) = bridge_fixture()?;
        let link = user.join("Documents");
        std::fs::remove_file(&link)?;
        std::os::unix::fs::symlink(
            temp.path()
                .join("intermediate/drive_c/users/steamuser/Documents"),
            &link,
        )?;
        changes.push(FolderChange {
            old_path: temp.path().join("intermediate"),
            ..changes[0].clone()
        });
        repair_links(temp.path(), &game, &changes)?;
        assert_eq!(
            std::fs::read_link(link)?,
            temp.path()
                .join("new-prefix/drive_c/users/steamuser/Documents")
        );
        Ok(())
    }

    // @variants: snap
    #[tokio::test]
    async fn excludes_recovery_until_an_existing_operation_releases_its_lease() {
        let lock = Arc::new(RwLock::new(()));
        let activity = lock.clone().try_read_owned().unwrap();
        assert!(lock.clone().try_write_owned().is_err());
        drop(activity);
        let recovery = lock.clone().try_write_owned().unwrap();
        assert!(lock.clone().try_read_owned().is_err());
        drop(recovery);
        assert!(lock.try_read_owned().is_ok());
    }

    // @variants: snap
    #[test]
    fn repairs_only_the_expected_link_and_can_repeat() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let link = temp.path().join("Documents");
        std::os::unix::fs::symlink("/old/Documents", &link)?;
        for _ in 0..2 {
            replace_owned_link(
                &link,
                Path::new("/old/Documents"),
                Path::new("/new/Documents"),
            )?;
        }
        assert_eq!(std::fs::read_link(&link)?, Path::new("/new/Documents"));
        assert!(
            replace_owned_link(
                &link,
                Path::new("/old/Documents"),
                Path::new("/other/Documents")
            )
            .is_err()
        );
        assert_eq!(std::fs::read_link(&link)?, Path::new("/new/Documents"));
        Ok(())
    }

    // @variants: snap
    #[test]
    fn rejects_a_binding_that_escapes_through_a_link() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("root");
        std::fs::create_dir(&root)?;
        std::os::unix::fs::symlink(temp.path(), root.join("outside"))?;
        assert!(validate_bound_folder(&root, &root.join("outside"), FolderRole::Game).is_err());
        Ok(())
    }
}
