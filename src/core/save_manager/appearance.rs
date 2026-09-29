use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::{
    game::mass_effect::{Target, appearance::save::Document},
    generations,
    tracker::Tracker,
};
use crate::models::game::Game;
use crate::utils::location::FolderRole;

use super::SaveSetId;

const MAX_FILE: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct SaveEntry {
    pub relative: PathBuf,
    pub label: String,
}
#[derive(Debug, Clone)]
pub(crate) struct Listing {
    pub entries: Vec<SaveEntry>,
    pub owner: String,
    pub errors: Vec<String>,
}
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub game: Game,
    pub owner: SaveSetId,
    pub owner_label: String,
    pub relative: PathBuf,
    pub document: Document,
    root: PathBuf,
    binding: (i64, PathBuf, PathBuf),
    identity: FileIdentity,
}

fn target(game: &Game) -> Result<Target> {
    [Target::Le1, Target::Le2, Target::Le3]
        .into_iter()
        .find(|t| t.game_id() == game.id)
        .context("Appearance editing is available only for Legendary Edition games")
}

async fn binding(tracker: &Tracker, game: &Game) -> Result<(i64, PathBuf, PathBuf)> {
    let record = tracker
        .folder_location(&game.id, FolderRole::Prefix)
        .await?;
    let relative = record
        .bindings
        .iter()
        .find(|b| b.game_id == game.id && b.role == FolderRole::Prefix)
        .context("Save folder binding is missing")?
        .relative
        .clone();
    Ok((record.id, record.selection.root, relative))
}

async fn owner_label(tracker: &Tracker, owner: &SaveSetId) -> Result<String> {
    if let Some(id) = owner.profile_id() {
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM profiles WHERE id=? AND game_id=?")
                .bind(id)
                .bind(owner.game_id())
                .fetch_optional(&tracker.pool)
                .await?;
        Ok(format!(
            "Profile: {}",
            name.context("The live save owner no longer exists")?
        ))
    } else {
        Ok("Global saves".into())
    }
}

fn root(game: &Game) -> Result<PathBuf> {
    let root = super::validate_live_save_access_with(game, |prefix| {
        crate::utils::snap::validate_selected_folder(
            prefix,
            crate::utils::snap::SelectedFolderKind::WinePrefix,
        )
    })?;
    let root = resolve_save_root(
        game.wine_prefix.as_deref().context("No Wine prefix")?,
        &root,
        crate::utils::snap::is_snap(),
    )?;
    ensure!(
        fs::symlink_metadata(&root)?.is_dir(),
        "The live save folder is not a directory"
    );
    Ok(root)
}

fn resolve_save_root(prefix: &Path, save: &Path, confined: bool) -> Result<PathBuf> {
    ensure!(
        prefix.join("drive_c").is_dir() || !prefix.join("pfx/drive_c").is_dir(),
        "The configured Wine prefix points to a Steam compatdata folder instead of its pfx child. Open Settings → Manage Games, change the Wine prefix and select the containing numbered folder; Deployd will use pfx inside it"
    );
    let resolved = fs::canonicalize(save).with_context(|| {
        format!(
            "Cannot resolve the live save directory '{}'. Check the configured Wine prefix and its Documents folder",
            save.display()
        )
    })?;
    if confined {
        let prefix = fs::canonicalize(prefix).context("Cannot resolve the granted Wine prefix")?;
        ensure!(
            resolved.starts_with(prefix),
            "The save directory resolves outside the granted Wine prefix; editing that location is not supported in the Snap"
        );
        Ok(save.to_path_buf())
    } else {
        Ok(resolved)
    }
}

fn resolve(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "Invalid relative save path"
    );
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "Save paths cannot contain symbolic links"
        );
    }
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    changed: i64,
    changed_nanos: i64,
}

fn read(path: &Path) -> Result<(Vec<u8>, FileIdentity)> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let before = file.metadata()?;
    ensure!(
        before.is_file() && before.nlink() == 1 && before.len() <= MAX_FILE,
        "Save must be a regular, unlinked file of at most 64 MiB"
    );
    let mut bytes = Vec::new();
    (&mut file).take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        bytes.len() as u64 == before.len()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "Save changed while reading; reopen it"
    );
    Ok((
        bytes,
        FileIdentity {
            device: before.dev(),
            inode: before.ino(),
            changed: before.ctime(),
            changed_nanos: before.ctime_nsec(),
        },
    ))
}

pub(crate) async fn list(tracker: &Tracker, game: &Game) -> Result<Listing> {
    let _lease = crate::core::location_recovery::activity_lock()
        .try_write_owned()
        .context("Finish the current game or save operation first")?;
    ensure_ready(tracker, game).await?;
    recover(tracker, game).await?;
    let owner = generations::session::live_saves(tracker, game).await?;
    let owner = owner_label(tracker, &owner).await?;
    let game = game.clone();
    tokio::task::spawn_blocking(move || {
        let _lease = _lease;
        let target = target(&game)?;
        let root = root(&game)?;
        let mut result = Listing {
            entries: Vec::new(),
            owner,
            errors: Vec::new(),
        };
        for (index, entry) in walkdir::WalkDir::new(&root)
            .follow_links(false)
            .min_depth(1)
            .into_iter()
            .enumerate()
        {
            ensure!(index < 100_000, "Too many files in the save folder");
            let entry = entry?;
            if !entry.file_type().is_file()
                || !entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("pcsav"))
            {
                continue;
            }
            let relative = entry.path().strip_prefix(&root)?.to_path_buf();
            let parsed = read(entry.path()).and_then(|(bytes, _)| Document::read(&bytes, target));
            match parsed {
                Ok(doc) => result.entries.push(SaveEntry {
                    label: format!(
                        "{} · level {} · {} · {}",
                        doc.name,
                        doc.level,
                        if doc.female {
                            "Female Shepard"
                        } else {
                            "Male Shepard"
                        },
                        relative.display()
                    ),
                    relative,
                }),
                Err(error) => result
                    .errors
                    .push(format!("{}: {error:#}", relative.display())),
            }
        }
        result.entries.sort_by(|a, b| a.relative.cmp(&b.relative));
        Ok(result)
    })
    .await?
}

pub(crate) async fn open(tracker: &Tracker, game: Game, relative: PathBuf) -> Result<Session> {
    let lease = crate::core::location_recovery::activity_lock()
        .try_write_owned()
        .context("Finish the current game or save operation first")?;
    ensure_ready(tracker, &game).await?;
    recover(tracker, &game).await?;
    let owner = generations::session::live_saves(tracker, &game).await?;
    let owner_label = owner_label(tracker, &owner).await?;
    let binding = binding(tracker, &game).await?;
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let root = root(&game)?;
        let (bytes, identity) = read(&resolve(&root, &relative)?)?;
        let document = Document::read(&bytes, target(&game)?)?;
        Ok(Session {
            game,
            owner,
            owner_label,
            relative,
            document,
            root,
            binding,
            identity,
        })
    })
    .await?
}

#[derive(Serialize, Deserialize)]
struct Journal {
    version: u32,
    owner: SaveSetId,
    relative: PathBuf,
    staging: String,
    before: String,
    after: String,
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn journal_path(game: &Game) -> Result<PathBuf> {
    Ok(super::game_root(&game.id)?.join("appearance-edit.json"))
}

pub(crate) fn ensure_idle(game: &Game) -> Result<()> {
    ensure!(
        !journal_path(game)?.try_exists()?,
        "An interrupted appearance edit needs recovery; reopen this game in Deployd"
    );
    Ok(())
}

fn durable(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::File::open(path.parent().context("Missing parent directory")?)?.sync_all()?;
    Ok(())
}

pub(crate) async fn save(tracker: Tracker, session: Session) -> Result<Session> {
    let lease = crate::core::location_recovery::activity_lock()
        .try_write_owned()
        .context("Finish the current game or save operation first")?;
    tokio::spawn(async move {
        ensure_ready(&tracker, &session.game).await?;
        ensure_idle(&session.game)?;
        ensure!(
            generations::session::live_saves(&tracker, &session.game).await? == session.owner,
            "Live save ownership changed; reopen the editor"
        );
        ensure!(
            binding(&tracker, &session.game).await? == session.binding,
            "Save folder binding changed; reopen the editor"
        );
        let current = tracker
            .load_persisted_games()
            .await?
            .into_iter()
            .find(|g| g.id == session.game.id)
            .context("Game was removed")?;
        ensure!(
            current.wine_prefix == session.game.wine_prefix,
            "Wine prefix changed; reopen the editor"
        );
        let check = session.clone();
        tokio::task::spawn_blocking(move || check_current(&check)).await??;
        let backup = super::create_manual_backup(
            &session.game,
            &session.owner,
            "Before appearance edit".into(),
        )
        .await?;
        let backup_root = super::backup_root(&session.game.id, &backup.backup_id)?;
        let next = session.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            for entry in walkdir::WalkDir::new(&backup_root).contents_first(true) {
                let entry = entry?;
                fs::File::open(entry.path())?.sync_all()?;
            }
            fs::File::open(backup_root.parent().context("Backup parent missing")?)?.sync_all()?;
            commit(next)
        })
        .await?
    })
    .await?
}

async fn ensure_ready(tracker: &Tracker, game: &Game) -> Result<()> {
    tracker.ensure_no_mele_journal(&game.id).await?;
    ensure!(
        !tokio::fs::try_exists(super::game_root(&game.id)?.join("transition.json")).await?,
        "Save transition recovery must finish before appearance editing; reopen this game"
    );
    Ok(())
}

fn check_current(session: &Session) -> Result<()> {
    ensure!(
        root(&session.game)? == session.root,
        "Save location changed; reopen the editor"
    );
    let (bytes, identity) = read(&resolve(&session.root, &session.relative)?)?;
    ensure!(
        identity == session.identity && bytes == session.document.original(),
        "Save changed outside Deployd; reopen it before editing"
    );
    Ok(())
}

fn commit(mut session: Session) -> Result<Session> {
    check_current(&session)?;
    let output = session.document.write()?;
    let path = resolve(&session.root, &session.relative)?;
    let staging = format!(".deployd-appearance-{}.tmp", uuid::Uuid::new_v4());
    let staged = path.parent().context("Save parent missing")?.join(&staging);
    let journal = Journal {
        version: 1,
        owner: session.owner.clone(),
        relative: session.relative.clone(),
        staging,
        before: hash(session.document.original()),
        after: hash(&output),
    };
    let journal_path = journal_path(&session.game)?;
    fs::create_dir_all(journal_path.parent().context("Journal parent missing")?)?;
    let mut record =
        tempfile::NamedTempFile::new_in(journal_path.parent().context("Journal parent missing")?)?;
    record.write_all(&serde_json::to_vec(&journal)?)?;
    record.as_file().sync_all()?;
    record.persist_noclobber(&journal_path)?;
    fs::File::open(journal_path.parent().context("Journal parent missing")?)?.sync_all()?;
    let result = (|| -> Result<()> {
        durable(&staged, &output)?;
        fs::set_permissions(
            &staged,
            fs::Permissions::from_mode(fs::metadata(&path)?.permissions().mode() & 0o777),
        )?;
        fs::File::open(&staged)?.sync_all()?;
        check_current(&session)?;
        fs::rename(&staged, &path)?;
        fs::File::open(path.parent().context("Save parent missing")?)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        return Err(error).context("Appearance write interrupted; reopen this game to recover. The pre-edit backup was retained");
    }
    finish(&session.root, &journal_path, &journal)?;
    let (bytes, identity) = read(&path)?;
    session.document = Document::read(&bytes, target(&session.game)?)?;
    session.identity = identity;
    Ok(session)
}

fn finish(root: &Path, path: &Path, journal: &Journal) -> Result<()> {
    ensure!(
        journal.version == 1
            && journal.staging.starts_with(".deployd-appearance-")
            && Path::new(&journal.staging).components().count() == 1,
        "Invalid appearance recovery journal"
    );
    let live = resolve(root, &journal.relative)?;
    let actual = hash(&read(&live)?.0);
    ensure!(
        actual == journal.before || actual == journal.after,
        "External save edits were preserved; appearance recovery is blocked"
    );
    let staged = live
        .parent()
        .context("Missing save parent")?
        .join(&journal.staging);
    match fs::symlink_metadata(&staged) {
        Ok(_) => {
            let staged_bytes = read(&staged)?.0;
            ensure!(
                actual == journal.before || hash(&staged_bytes) == journal.after,
                "Unexpected appearance staging was preserved; recovery is blocked"
            );
            fs::remove_file(&staged)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Cannot inspect appearance staging"),
    }
    fs::File::open(live.parent().context("Missing save parent")?)?.sync_all()?;
    fs::remove_file(path)?;
    fs::File::open(path.parent().context("Missing journal parent")?)?.sync_all()?;
    Ok(())
}

pub(crate) async fn recover(tracker: &Tracker, game: &Game) -> Result<()> {
    let path = journal_path(game)?;
    if !tokio::fs::try_exists(&path).await? {
        return Ok(());
    }
    let recovery_game = game.clone();
    let path_copy = path.clone();
    let (journal, root) = tokio::task::spawn_blocking(move || -> Result<(Journal, PathBuf)> {
        let bytes = read(&path_copy)?.0;
        ensure!(bytes.len() < 65536, "Appearance journal is too large");
        Ok((serde_json::from_slice(&bytes)?, root(&recovery_game)?))
    })
    .await??;
    ensure!(
        generations::session::live_saves(tracker, game).await? == journal.owner,
        "Live save ownership changed; appearance recovery is blocked"
    );
    tokio::task::spawn_blocking(move || finish(&root, &path, &journal)).await?
}

#[cfg(test)]
mod tests;
