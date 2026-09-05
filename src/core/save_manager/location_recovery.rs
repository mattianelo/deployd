use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::models::game::Game;
use crate::utils::location::{FolderChange, FolderRole, rebase};

use super::{TransitionJournal, game_root, read_json, validate_live_save_access, write_json};

pub(crate) async fn rebase_location_journal(game: &Game, changes: &[FolderChange]) -> Result<bool> {
    let journal_path = game_root(&game.id)?.join("transition.json");
    let metadata = match tokio::fs::symlink_metadata(&journal_path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("Cannot inspect the interrupted save journal"),
    };
    if !metadata.is_file() {
        bail!("The interrupted save journal is not a regular file; it was preserved")
    }
    let Some(first) = changes
        .iter()
        .find(|change| change.role == FolderRole::Prefix)
    else {
        return Ok(false);
    };
    let live = tokio::task::spawn_blocking({
        let game = game.clone();
        move || validate_live_save_access(&game)
    })
    .await??;
    let journal: TransitionJournal = read_json(&journal_path).await?;
    let change = changes
        .iter()
        .filter(|change| change.role == FolderRole::Prefix)
        .find(|change| {
            rebase(&live, &change.new_path, &change.old_path)
                .is_ok_and(|old| old.as_ref() == Some(&journal.live_path))
        })
        .unwrap_or(first);
    let journal = rebase_journal(journal, &game.id, change, &live)?;
    let prefix = change.new_path.clone();
    let parent = live
        .parent()
        .context("The live save path has no parent")?
        .to_path_buf();
    tokio::task::spawn_blocking(move || {
        crate::core::location_recovery::require_contained(&prefix, &parent)
    })
    .await??;
    for path in [&journal.live_path, &journal.rollback_path] {
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) if !metadata.is_dir() => {
                bail!("An interrupted save directory was replaced; it was preserved")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect interrupted save directories"),
        }
    }
    write_json(&journal_path, &journal)
        .await
        .context("Could not rebase the interrupted save journal")?;
    Ok(true)
}

fn rebase_journal(
    mut journal: TransitionJournal,
    game_id: &str,
    change: &FolderChange,
    live: &Path,
) -> Result<TransitionJournal> {
    if journal.source.game_id() != game_id
        || journal.target.game_id() != game_id
        || change.game_id != game_id
    {
        bail!("The interrupted save journal belongs to a different game; it was preserved");
    }
    let previous_live = rebase(live, &change.new_path, &change.old_path)?
        .context("Save directory is outside the authorized prefix")?;
    let original_live = journal.live_path.clone();
    if original_live != previous_live && original_live != live {
        bail!("The interrupted save journal targets an unexpected directory; it was preserved")
    }
    let name = journal
        .rollback_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid save rollback directory")?;
    let suffix = name
        .strip_prefix(".deployd-save-rollback-")
        .context("Unrecognized save rollback directory; it was preserved")?;
    uuid::Uuid::parse_str(suffix)
        .context("Invalid save rollback identifier; journal was preserved")?;
    if journal.rollback_path.parent() != original_live.parent() {
        bail!("Save rollback directory is outside the live save parent; journal was preserved")
    }
    let parent = live.parent().context("Save directory has no parent")?;
    journal.rollback_path = parent.join(name);
    journal.live_path = live.to_path_buf();
    Ok(journal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::save_manager::{SaveSetId, TransitionPhase};

    fn fixture() -> (TransitionJournal, FolderChange) {
        let game_id = "skyrim".to_string();
        let change = FolderChange {
            game_id: game_id.clone(),
            role: FolderRole::Prefix,
            old_path: "/doc/old/pfx".into(),
            new_path: "/doc/new/pfx".into(),
        };
        let journal = TransitionJournal {
            source: SaveSetId::Global {
                game_id: game_id.clone(),
            },
            target: SaveSetId::Global { game_id },
            live_path: "/doc/old/pfx/Documents/Saves".into(),
            rollback_path: format!(
                "/doc/old/pfx/Documents/.deployd-save-rollback-{}",
                uuid::Uuid::new_v4()
            )
            .into(),
            phase: TransitionPhase::LivePending,
        };
        (journal, change)
    }

    // @variants: snap
    #[test]
    fn rebases_the_save_journal_without_changing_its_transition() -> Result<()> {
        let (journal, change) = fixture();
        let live = Path::new("/doc/new/pfx/Documents/Saves");
        let journal = rebase_journal(journal, "skyrim", &change, live)?;
        assert_eq!(journal.live_path, live);
        assert_eq!(journal.rollback_path.parent(), live.parent());
        let journal = rebase_journal(journal, "skyrim", &change, live)?;
        assert_eq!(journal.phase, TransitionPhase::LivePending);
        Ok(())
    }

    // @variants: snap
    #[test]
    fn rejects_foreign_or_unexpected_save_journals() {
        let (journal, change) = fixture();
        assert!(
            rebase_journal(
                journal,
                "other",
                &change,
                Path::new("/doc/new/pfx/Documents/Saves")
            )
            .is_err()
        );
        let (mut journal, change) = fixture();
        journal.rollback_path = "/doc/old/pfx/unrelated".into();
        assert!(
            rebase_journal(
                journal,
                "skyrim",
                &change,
                Path::new("/doc/new/pfx/Documents/Saves")
            )
            .is_err()
        );
    }

    // @variants: snap
    #[tokio::test]
    async fn restores_original_saves_from_a_rebased_interrupted_transition() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let prefix = temp.path().join("new");
        let live = prefix.join("Documents/Saves");
        let rollback_name = format!(".deployd-save-rollback-{}", uuid::Uuid::new_v4());
        let rollback = prefix.join("Documents").join(&rollback_name);
        std::fs::create_dir_all(&live)?;
        std::fs::create_dir(&rollback)?;
        std::fs::write(live.join("save"), "partial transition")?;
        std::fs::write(rollback.join("save"), "original saves")?;
        let change = FolderChange {
            game_id: "game".to_string(),
            role: FolderRole::Prefix,
            old_path: temp.path().join("old"),
            new_path: prefix,
        };
        let source = SaveSetId::Global {
            game_id: "game".to_string(),
        };
        let journal = TransitionJournal {
            source: source.clone(),
            target: SaveSetId::Profile {
                game_id: "game".to_string(),
                profile_id: "other".to_string(),
            },
            live_path: change.old_path.join("Documents/Saves"),
            rollback_path: change.old_path.join("Documents").join(rollback_name),
            phase: TransitionPhase::LiveRestored,
        };
        let journal = rebase_journal(journal, "game", &change, &live)?;
        let journal_path = temp.path().join("transition.json");
        write_json(&journal_path, &journal).await?;
        crate::core::save_manager::recover_transition_journal(&journal_path, &live, &source)
            .await?;
        assert_eq!(
            std::fs::read_to_string(live.join("save"))?,
            "original saves"
        );
        assert!(!journal_path.exists());
        Ok(())
    }
}
