use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::tracker::Tracker;
use crate::models::game::Game;
use crate::utils::location::{FolderRole, resolve_relative};

use super::components::{self, Component};
use super::journal::{Identity, Operation, State, files};
use super::operation::Control;

pub(crate) mod generations;

const BINK: &str = "bink2w64.dll";
const ORIGINAL: &str = "bink2w64_original.dll";
const EXE: &str = "MassEffectLauncher.exe";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Family {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) mods: Vec<super::launcher::Entry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) originals: BTreeMap<String, Option<Identity>>,
    version: u32,
    original: Identity,
    owners: BTreeSet<String>,
    installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Change {
    #[serde(default)]
    pub(crate) launcher_edit: bool,
    pub(crate) location_id: i64,
    pub(crate) previous: Family,
    pub(crate) desired: Family,
    missing: Vec<String>,
    operations: Vec<Operation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Plan {
    pub(super) root: PathBuf,
    pub(super) change: Change,
    recorded: bool,
}

fn required(state: Option<&State>) -> bool {
    state
        .and_then(|state| state.recipe.as_ref())
        .is_some_and(|recipe| {
            recipe
                .components
                .iter()
                .any(|selection| selection.component == Component::BinkProxy)
        })
}

fn proxy() -> Identity {
    components::proxy_identity()
}

impl Family {
    pub(crate) fn validate(&self) -> Result<()> {
        self.original.validate()?;
        ensure!(
            serde_json::to_vec(self)?.len() <= 16 * 1024 * 1024,
            "Shared launcher record exceeds its size limit"
        );
        ensure!(
            (1..=2).contains(&self.version)
                && (!self.installed || self.support_required())
                && self.original.size > 0
                && self.original.size <= 16 * 1024 * 1024
                && self.original != proxy()
                && self.owners.iter().all(|id| [
                    "mass-effect-le1",
                    "mass-effect-le2",
                    "mass-effect-le3"
                ]
                .contains(&id.as_str())),
            "Invalid MELE launcher ownership record"
        );
        ensure!(
            self.version >= 2 || self.mods.is_empty() && self.originals.is_empty(),
            "Launcher mods require shared state version 2"
        );
        super::launcher::validate_entries(&self.mods)?;
        ensure!(
            self.originals.len() <= 10000,
            "Too many launcher restoration entries"
        );
        let mut original_paths = BTreeMap::new();
        for (path, identity) in &self.originals {
            for prefix in Path::new(path)
                .ancestors()
                .filter(|path| !path.as_os_str().is_empty())
            {
                let prefix = prefix.to_str().context("Invalid launcher original path")?;
                let value = (prefix.to_owned(), prefix == path);
                ensure!(
                    original_paths
                        .insert(prefix.to_lowercase(), value.clone())
                        .is_none_or(|old| old == value),
                    "Launcher originals collide by case or file/directory type"
                );
            }
            super::launcher::destination(path)?;
            if let Some(identity) = identity {
                identity.validate()?;
            }
        }
        ensure!(
            self.mods
                .iter()
                .flat_map(|entry| &entry.files)
                .all(|file| self.originals.contains_key(&file.destination)),
            "Launcher mod has no recorded original"
        );
        Ok(())
    }

    fn support_required(&self) -> bool {
        !self.owners.is_empty() || self.mods.iter().any(|entry| entry.enabled)
    }

    fn files(&self) -> BTreeMap<String, Identity> {
        let mut files: BTreeMap<String, Identity> = self
            .originals
            .iter()
            .filter_map(|(path, identity)| {
                identity.clone().map(|identity| (path.clone(), identity))
            })
            .collect();
        for entry in self.mods.iter().filter(|entry| entry.enabled) {
            for file in &entry.files {
                files.insert(file.destination.clone(), file.identity.clone());
            }
        }
        files.insert(
            BINK.into(),
            if self.installed {
                proxy()
            } else {
                self.original.clone()
            },
        );
        if self.installed {
            files.insert(ORIGINAL.into(), self.original.clone());
        }
        files
    }
}

impl Change {
    pub(crate) fn validate(&self, game_id: &str, state: &State) -> Result<()> {
        self.previous.validate()?;
        self.desired.validate()?;
        ensure!(
            self.location_id > 0 && self.previous.original == self.desired.original,
            "MELE launcher baseline changed in a deployment journal"
        );
        ensure!(
            self.desired.installed == self.desired.support_required()
                && self.desired.owners.contains(game_id) == required(Some(state)),
            "MELE launcher ownership disagrees with the game deployment"
        );
        ensure!(
            self.launcher_edit
                || self.previous.mods == self.desired.mods
                    && self.previous.originals == self.desired.originals,
            "Game deployment cannot change the shared launcher-mod list"
        );
        ensure!(
            self.previous
                .originals
                .iter()
                .all(|(path, identity)| self.desired.originals.get(path) == Some(identity)),
            "Launcher restoration identities cannot be changed"
        );
        let mut old = self.previous.owners.clone();
        let mut new = self.desired.owners.clone();
        old.remove(game_id);
        new.remove(game_id);
        ensure!(
            old == new,
            "A game deployment cannot remove another game's launcher ownership"
        );
        ensure!(
            self.operations == self.operations()?,
            "MELE launcher journal operations disagree with ownership"
        );
        Ok(())
    }

    fn operations(&self) -> Result<Vec<Operation>> {
        let before = self.previous.files();
        let after = self.desired.files();
        let mut seen = BTreeSet::new();
        for path in &self.missing {
            ensure!(
                self.previous.installed && before.contains_key(path) && seen.insert(path),
                "Only managed launcher files can be repaired"
            );
        }
        let mut operations = Vec::new();
        for path in self.paths() {
            let before = if self.missing.iter().any(|entry| entry == &path) {
                None
            } else {
                before.get(&path).cloned()
            };
            let after = after.get(&path).cloned();
            if before != after {
                operations.push(Operation {
                    path,
                    before,
                    after,
                });
            }
        }
        Ok(operations)
    }

    fn paths(&self) -> BTreeSet<String> {
        [BINK.into(), ORIGINAL.into()]
            .into_iter()
            .chain(self.previous.originals.keys().cloned())
            .chain(self.desired.originals.keys().cloned())
            .collect()
    }

    pub(super) fn storage(&self, data: &Path, id: &str) -> PathBuf {
        data.join("mele-family-transactions")
            .join(self.location_id.to_string())
            .join(id)
    }

    pub(super) fn apply(&self, root: &Path, stage: &Path, control: &Control) -> Result<()> {
        for operation in &self.operations {
            files::replace(root, stage, operation, false, control)?;
        }
        self.verify(root, false, control)
    }

    pub(super) fn rollback(&self, root: &Path, stage: &Path) -> Result<()> {
        for operation in self.operations.iter().rev() {
            files::clear_temporary(root, stage, operation)?;
            files::replace(root, stage, operation, true, &Control::recovery())?;
        }
        self.verify(root, true, &Control::recovery())
    }

    pub(super) fn cleanup(&self, stage: &Path) -> Result<()> {
        files::cleanup_operations(stage, &self.operations)
    }

    pub(super) fn verify(&self, root: &Path, previous: bool, control: &Control) -> Result<()> {
        let expected = if previous {
            self.previous.files()
        } else {
            self.desired.files()
        };
        for path in self.paths() {
            let identity = if previous && self.missing.iter().any(|entry| entry == &path) {
                None
            } else {
                expected.get(&path)
            };
            files::verify(root, &path, identity, control)?;
        }
        Ok(())
    }
}

pub(super) async fn root(tracker: &Tracker, game: &Game, location_id: i64) -> Result<PathBuf> {
    let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
    ensure!(
        location.id == location_id,
        "MELE launcher folder binding changed; restore access before recovery"
    );
    let number = match game.id.as_str() {
        "mass-effect-le1" => 1,
        "mass-effect-le2" => 2,
        "mass-effect-le3" => 3,
        _ => anyhow::bail!("Unsupported MELE launcher target"),
    };
    ensure!(
        location
            .bindings
            .iter()
            .any(|binding| binding.game_id == game.id
                && binding.role == FolderRole::Game
                && binding.relative == Path::new(&format!("Game/ME{number}")))
            && resolve_relative(
                &location.selection.root,
                Path::new(&format!("Game/ME{number}"))
            )? == game.path,
        "MELE launcher support requires the shared Legendary Edition folder; add the installation as a group"
    );
    resolve_relative(&location.selection.root, Path::new("Game/Launcher"))
}

pub(super) async fn inspect(
    tracker: &Tracker,
    game: &Game,
    desired: &State,
    repair: bool,
    control: Control,
) -> Result<Option<Plan>> {
    let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
    let existing = tracker.mele_family(location.id).await?;
    if existing.is_none() && !required(Some(desired)) {
        return Ok(None);
    }
    let root = root(tracker, game, location.id).await?;
    let mut owners = existing
        .as_ref()
        .map(|family| family.owners.clone())
        .unwrap_or_default();
    for binding in &location.bindings {
        if binding.role == FolderRole::Game
            && binding.game_id != game.id
            && required(tracker.mele_deployment(&binding.game_id).await?.as_ref())
        {
            owners.insert(binding.game_id.clone());
        }
    }
    if required(Some(desired)) {
        owners.insert(game.id.clone());
    } else {
        owners.remove(&game.id);
    }
    let game_id = game.id.clone();
    let desired_state = desired.clone();
    tokio::task::spawn_blocking(move || {
        files::check_path(&root, EXE)?;
        let asi = root.join("ASI");
        match fs::symlink_metadata(&asi) {
            Ok(_) => {
                super::baseline::directory(&asi)?;
                let known = existing.as_ref().map(Family::files).unwrap_or_default();
                for entry in walkdir::WalkDir::new(&asi).follow_links(false) {
                    let entry = entry?;
                    ensure!(entry.file_type().is_file() || entry.file_type().is_dir(), "Launcher plugin folder contains links or special files");
                    if entry.file_type().is_file() {
                        let path = entry.path().strip_prefix(&root)?.to_str().context("Invalid launcher path")?;
                        ensure!(known.contains_key(path), "Unmanaged launcher plugins are present; reconcile them before enabling shared launcher support");
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect launcher plugins"),
        }
        ensure!(root.join(EXE).is_file(), "MELE launcher executable is missing; restore folder access or repair the game");
        let recorded = existing.is_some();
        let previous = match existing {
            Some(value) => value,
            None => {
                files::check_path(&root, BINK)?;
                let metadata = fs::symlink_metadata(root.join(BINK))?;
                ensure!(metadata.len() > 0 && metadata.len() <= 16 * 1024 * 1024, "Invalid launcher Bink library size");
                let mut bytes = Vec::new();
                fs::File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(root.join(BINK))?
                    .take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                ensure!(bytes.len() as u64 == metadata.len(), "Launcher Bink library changed during inspection");
                Family { mods: Vec::new(), originals: BTreeMap::new(), version: 1, original: Identity { size: bytes.len() as u64, sha256: format!("{:x}", Sha256::digest(&bytes)) }, owners: owners.iter().filter(|id| *id != &game_id).cloned().collect(), installed: false }
            }
        };
        previous.validate()?;
        let mut desired = previous.clone();
        desired.owners = owners;
        desired.installed = desired.support_required();
        let mut change = Change { launcher_edit: false, location_id: location.id, previous, desired, missing: Vec::new(), operations: Vec::new() };
        if repair && change.previous.installed {
            for path in change.paths() {
                if matches!(fs::symlink_metadata(root.join(&path)), Err(error) if error.kind() == std::io::ErrorKind::NotFound) {
                    change.missing.push(path);
                }
            }
        }
        change.verify(&root, true, &control)?;
        change.operations = change.operations()?;
        change.validate(&game_id, &desired_state)?;
        Ok(Some(Plan { root, change, recorded }))
    }).await.context("MELE launcher preflight worker failed")?
}

impl Plan {
    pub(super) async fn preserve(
        &self,
        tracker: &Tracker,
        data: &Path,
        control: Control,
    ) -> Result<()> {
        ensure!(
            !data.starts_with(&self.root) && !self.root.starts_with(data),
            "MELE launcher storage must be separate from the installation"
        );
        if self
            .change
            .operations
            .iter()
            .any(|operation| operation.after.as_ref() == Some(&proxy()))
        {
            components::cache_proxy(data.to_path_buf(), control.clone()).await?;
        }
        let source = self.root.clone();
        let original = self.change.previous.original.clone();
        let destination = data
            .join("mele-family-originals")
            .join(self.change.location_id.to_string());
        let recorded = self.recorded;
        tokio::task::spawn_blocking(move || {
            files::create_directory(&destination)?;
            if destination.join(BINK).try_exists()? {
                files::verify(&destination, BINK, Some(&original), &control)?;
            } else {
                ensure!(
                    !recorded,
                    "Preserved launcher original is missing; restore it before rebuilding"
                );
                files::copy(&source, BINK, &destination, BINK, &original, &control)?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        tracker
            .record_mele_family(self.change.location_id, &self.change.previous)
            .await
    }

    pub(super) fn stage(&self, data: &Path, id: &str, control: &Control) -> Result<()> {
        self.change.verify(&self.root, true, control)?;
        let stage = self.change.storage(data, id);
        files::create_directory(&stage)?;
        ensure!(
            !stage.starts_with(&self.root) && !self.root.starts_with(&stage),
            "Launcher staging overlaps the installation"
        );
        let originals = data
            .join("mele-family-originals")
            .join(self.change.location_id.to_string());
        let cache = data.join("mele-components/artifacts");
        let proxy_hash = proxy().sha256;
        for operation in &self.change.operations {
            for direction in ["old", "new"] {
                files::verify(
                    &self.root,
                    &files::temporary(&stage, operation, direction)?,
                    None,
                    control,
                )?;
            }
            if let Some(before) = &operation.before {
                files::copy(
                    &self.root,
                    &operation.path,
                    &stage,
                    &format!("old/{}", operation.path),
                    before,
                    control,
                )?;
            }
            if let Some(after) = &operation.after {
                let (input_root, input) = if operation.path == BINK && self.change.desired.installed
                {
                    (cache.as_path(), proxy_hash.as_str())
                } else if operation.path == BINK || operation.path == ORIGINAL {
                    (originals.as_path(), BINK)
                } else {
                    let source = self
                        .change
                        .desired
                        .mods
                        .iter()
                        .rev()
                        .filter(|entry| entry.enabled)
                        .find_map(|entry| {
                            entry
                                .files
                                .iter()
                                .find(|file| file.destination == operation.path)
                                .map(|file| (entry, file))
                        });
                    if let Some((entry, file)) = source {
                        let source_root = super::launcher::source_root(data, &entry.source_sha256);
                        files::copy(
                            &source_root,
                            &file.source,
                            &stage,
                            &format!("new/{}", operation.path),
                            after,
                            control,
                        )?;
                        continue;
                    }
                    (originals.as_path(), operation.path.as_str())
                };
                files::copy(
                    input_root,
                    input,
                    &stage,
                    &format!("new/{}", operation.path),
                    after,
                    control,
                )?;
            }
        }
        files::sync(&stage)
    }
}

pub(super) async fn inspect_recipe(
    tracker: &Tracker,
    game: &Game,
    recipe: &super::recipe::Recipe,
    repair: bool,
    control: Control,
) -> Result<Option<Plan>> {
    inspect(
        tracker,
        game,
        &State {
            removals: Default::default(),
            version: 2,
            generation: String::new(),
            profile: String::new(),
            files: Vec::new(),
            recipe: Some(recipe.clone()),
        },
        repair,
        control,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::game::mass_effect::{Target, recipe::Recipe};

    fn state(target: Target, enabled: bool) -> State {
        let recipe = Recipe {
            version: 2,
            target,
            language: "INT".into(),
            packages: Vec::new(),
            helper_version: None,
            backend_version: 1,
            components: if enabled {
                components::required(target)
            } else {
                Vec::new()
            },
        };
        State {
            removals: Default::default(),
            version: 2,
            generation: uuid::Uuid::new_v4().to_string(),
            profile: "profile".into(),
            files: Vec::new(),
            recipe: Some(recipe),
        }
    }

    fn family(owners: &[&str]) -> Family {
        Family {
            mods: Vec::new(),
            originals: BTreeMap::new(),
            version: 1,
            original: Identity {
                size: 8,
                sha256: "a".repeat(64),
            },
            owners: owners.iter().map(|id| (*id).into()).collect(),
            installed: !owners.is_empty(),
        }
    }

    fn change(previous: Family, desired: Family) -> Result<Change> {
        let mut change = Change {
            launcher_edit: false,
            location_id: 1,
            previous,
            desired,
            missing: Vec::new(),
            operations: Vec::new(),
        };
        change.operations = change.operations()?;
        Ok(change)
    }

    // @variants: both
    #[test]
    fn retains_launcher_until_the_last_game_is_restored() -> Result<()> {
        let installed = family(&["mass-effect-le1", "mass-effect-le2", "mass-effect-le3"]);
        let one_removed = family(&["mass-effect-le2", "mass-effect-le3"]);
        let transition = change(installed, one_removed)?;
        transition.validate("mass-effect-le1", &state(Target::Le1, false))?;
        assert!(transition.operations.is_empty());
        let transition = change(family(&["mass-effect-le3"]), family(&[]))?;
        transition.validate("mass-effect-le3", &state(Target::Le3, false))?;
        assert_eq!(transition.operations.len(), 2);
        assert_eq!(
            transition.operations[0].after,
            Some(transition.previous.original.clone())
        );
        assert!(transition.operations[1].after.is_none());
        Ok(())
    }

    // @variants: both
    #[test]
    fn adopts_existing_game_owners_without_claiming_launcher_files() -> Result<()> {
        let mut previous = family(&["mass-effect-le2"]);
        previous.installed = false;
        let transition = change(previous, family(&["mass-effect-le1", "mass-effect-le2"]))?;
        transition.validate("mass-effect-le1", &state(Target::Le1, true))?;
        assert_eq!(
            transition.operations[0].before,
            Some(transition.previous.original.clone())
        );
        assert!(transition.operations[1].before.is_none());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_foreign_ownership_and_tampered_launcher_operations() -> Result<()> {
        let mut transition = change(family(&["mass-effect-le1", "mass-effect-le2"]), family(&[]))?;
        assert!(
            transition
                .validate("mass-effect-le1", &state(Target::Le1, false))
                .is_err()
        );
        transition = change(family(&[]), family(&["mass-effect-le1"]))?;
        transition.operations[0].path = "../launcher/other.dll".into();
        assert!(
            transition
                .validate("mass-effect-le1", &state(Target::Le1, true))
                .is_err()
        );
        transition = change(family(&[]), family(&["mass-effect-le1"]))?;
        transition.desired.original = proxy();
        assert!(
            transition
                .validate("mass-effect-le1", &state(Target::Le1, true))
                .is_err()
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn repairs_only_missing_owned_launcher_files() -> Result<()> {
        let mut transition = change(family(&["mass-effect-le1"]), family(&["mass-effect-le1"]))?;
        transition.missing.push(BINK.into());
        transition.operations = transition.operations()?;
        transition.validate("mass-effect-le1", &state(Target::Le1, true))?;
        assert_eq!(transition.operations.len(), 1);
        assert!(transition.operations[0].before.is_none());
        transition.missing.push("other.dll".into());
        assert!(transition.operations().is_err());
        let mut unowned = change(family(&[]), family(&["mass-effect-le1"]))?;
        unowned.missing.push(BINK.into());
        assert!(unowned.operations().is_err());
        Ok(())
    }
    // @variants: both
    #[tokio::test]
    async fn resolves_only_the_shared_granted_launcher_and_preserves_external_files() -> Result<()>
    {
        use crate::models::game::{GameConfig, GameEngine};
        use crate::utils::location::{FolderSelection, SelectedLocation};
        let temp = tempfile::tempdir()?;
        let family_root = temp.path().join("selected");
        let launcher = family_root.join("Game/Launcher");
        fs::create_dir_all(&launcher)?;
        fs::write(launcher.join(EXE), b"launcher")?;
        fs::write(launcher.join(BINK), b"original")?;
        let mut game = Game {
            id: "mass-effect-le1".into(),
            title: "LE1".into(),
            path: family_root.join("Game/ME1"),
            data_subdir: "BioGame".into(),
            engine: GameEngine::MassEffect,
            wine_prefix: None,
        };
        let tracker = Tracker::open(&format!(
            "sqlite://{}?mode=rwc",
            temp.path().join("db").display()
        ))
        .await?
        .tracker;
        tracker
            .persist_game_configs(
                &[GameConfig {
                    game: game.clone(),
                    custom: true,
                    locations: vec![FolderSelection {
                        role: FolderRole::Game,
                        location: SelectedLocation {
                            root: family_root.clone(),
                            host_hint: None,
                        },
                        relative: "Game/ME1".into(),
                    }],
                }],
                &[],
            )
            .await?;
        let desired = state(Target::Le1, true);
        let plan = inspect(&tracker, &game, &desired, false, Control::recovery())
            .await?
            .context("missing plan")?;
        assert_eq!(plan.root, launcher);
        assert!(!plan.recorded);
        fs::create_dir(launcher.join("ASI"))?;
        fs::write(launcher.join("ASI/unknown.asi"), b"unmanaged plugin")?;
        assert!(
            inspect(&tracker, &game, &desired, false, Control::recovery())
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(launcher.join("ASI/unknown.asi"))?,
            b"unmanaged plugin"
        );
        fs::remove_file(launcher.join("ASI/unknown.asi"))?;

        tracker
            .record_mele_family(plan.change.location_id, &plan.change.previous)
            .await?;
        fs::write(launcher.join(ORIGINAL), b"unmanaged backing")?;
        assert!(
            inspect(&tracker, &game, &desired, true, Control::recovery())
                .await
                .is_err()
        );
        assert_eq!(fs::read(launcher.join(ORIGINAL))?, b"unmanaged backing");
        fs::remove_file(launcher.join(ORIGINAL))?;
        fs::rename(&launcher, family_root.join("saved-launcher"))?;
        std::os::unix::fs::symlink(family_root.join("saved-launcher"), &launcher)?;
        assert!(
            inspect(&tracker, &game, &desired, false, Control::recovery())
                .await
                .is_err()
        );
        fs::remove_file(&launcher)?;
        fs::rename(family_root.join("saved-launcher"), &launcher)?;
        let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
        let new_root = temp.path().join("reselected");
        fs::rename(&family_root, &new_root)?;
        let repair = tracker
            .commit_location_recovery(
                &location,
                &SelectedLocation {
                    root: new_root.clone(),
                    host_hint: None,
                },
                true,
            )
            .await?;
        tracker.finish_location_repair(&repair).await?;
        game.path = new_root.join("Game/ME1");
        assert_eq!(
            root(&tracker, &game, location.id).await?,
            new_root.join("Game/Launcher")
        );
        assert_eq!(
            tracker.mele_family(location.id).await?,
            Some(plan.change.previous)
        );
        game.path = new_root.join("Game/ME2");
        assert!(root(&tracker, &game, location.id).await.is_err());
        Ok(())
    }
}

impl Family {
    pub(crate) fn validate_extension(&self, desired: &Self) -> Result<()> {
        ensure!(
            self.original == desired.original
                && self.owners == desired.owners
                && self.installed == desired.installed
                && self.mods == desired.mods
                && self
                    .originals
                    .iter()
                    .all(|(path, identity)| desired.originals.get(path) == Some(identity)),
            "Launcher originals may only be extended without changing installed mods"
        );
        Ok(())
    }
}

pub(super) async fn edit(
    tracker: &Tracker,
    game: &Game,
    entries: Vec<super::launcher::Entry>,
    data: &Path,
    control: Control,
) -> Result<Plan> {
    super::launcher::validate_entries(&entries)?;
    let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
    if tracker.mele_family(location.id).await?.is_none() {
        let target = super::library::target(game)?;
        let recipe = super::recipe::Recipe {
            version: 2,
            target,
            language: "INT".into(),
            packages: Vec::new(),
            backend_version: 1,
            helper_version: None,
            components: components::required(target),
        };
        let plan = inspect_recipe(tracker, game, &recipe, false, control.clone())
            .await?
            .context("Missing shared launcher plan")?;
        plan.preserve(tracker, data, control.clone()).await?;
    }
    let previous = tracker
        .mele_family(location.id)
        .await?
        .context("Missing shared launcher state")?;
    let root = root(tracker, game, location.id).await?;
    let work = previous.clone();
    let originals = data
        .join("mele-family-originals")
        .join(location.id.to_string());
    let (extended, mut plan) = {
        let root = root.clone();
        let control = control.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            let mut extended = work.clone();
            extended.version = 2;
            for file in entries.iter().flat_map(|entry| &entry.files) {
                if extended.originals.contains_key(&file.destination) { continue; }
                files::check_path(&root, &file.destination)?;
                let identity = match fs::symlink_metadata(root.join(&file.destination)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                    Ok(metadata) => {
                        ensure!(metadata.is_file(), "Launcher original is not a regular file");
                        let mut input = fs::File::options().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(root.join(&file.destination))?;
                        let mut hash = Sha256::new(); let mut size = 0u64; let mut buffer = [0u8; 128 * 1024];
                        loop { control.check()?; let count = input.read(&mut buffer)?; if count == 0 { break; } hash.update(&buffer[..count]); size += count as u64; }
                        ensure!(size == metadata.len(), "Launcher original changed during preservation");
                        let identity = Identity { size, sha256: format!("{:x}", hash.finalize()) };
                        if originals.join(&file.destination).try_exists()? { files::verify(&originals, &file.destination, Some(&identity), &control)?; }
                        else { files::copy(&root, &file.destination, &originals, &file.destination, &identity, &control)?; }
                        Some(identity)
                    }
                };
                extended.originals.insert(file.destination.clone(), identity);
            }
            extended.validate()?;
            let mut desired = extended.clone(); desired.mods = entries; desired.installed = desired.support_required(); desired.validate()?;
            let mut change = Change { launcher_edit: true, location_id: location.id, previous: extended.clone(), desired, missing: Vec::new(), operations: Vec::new() };
            for (path, _) in change.previous.files() {
                if path != BINK && path != ORIGINAL && !change.previous.mods.iter().filter(|entry| entry.enabled).any(|entry| entry.files.iter().any(|file| file.destination == path)) { continue; }
                if matches!(fs::symlink_metadata(root.join(&path)), Err(error) if error.kind() == std::io::ErrorKind::NotFound) && change.previous.installed { change.missing.push(path); }
            }
            change.verify(&root, true, &control)?;
            change.operations = change.operations()?;
            Ok((extended, Plan { root, change, recorded: true }))
        }).await??
    };
    if extended != previous {
        tracker
            .extend_mele_family(location.id, &previous, &extended)
            .await?;
    }
    // Ownership remains with the deployed games even if a different profile is selected.
    let state = tracker.mele_deployment(&game.id).await?;
    if required(state.as_ref()) {
        plan.change.desired.owners.insert(game.id.clone());
    }
    plan.change.desired.installed = plan.change.desired.support_required();
    plan.change.operations = plan.change.operations()?;
    Ok(plan)
}
