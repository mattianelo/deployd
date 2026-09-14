use super::*;
use crate::core::generations::content::Identity as Content;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Revision {
    pub(crate) location: i64,
    mods: Vec<super::super::launcher::Entry>,
    original: Identity,
    originals: BTreeMap<String, Option<Identity>>,
    installed: bool,
    runtime: Option<Identity>,
}

impl Revision {
    pub(crate) fn capture(location: i64, family: &Family) -> Result<Self> {
        family.validate()?;
        ensure!(location > 0, "Invalid shared launcher binding");
        Ok(Self {
            location,
            mods: family.mods.clone(),
            original: family.original.clone(),
            originals: family
                .originals
                .iter()
                .filter(|(path, _)| {
                    family
                        .mods
                        .iter()
                        .flat_map(|entry| &entry.files)
                        .any(|file| &file.destination == *path)
                })
                .map(|(path, identity)| (path.clone(), identity.clone()))
                .collect(),
            installed: family.installed,
            runtime: family.installed.then(proxy),
        })
    }

    pub(crate) fn id(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(self.location > 0, "Invalid historical launcher binding");
        self.original.validate()?;
        ensure!(
            self.installed == self.runtime.is_some(),
            "Historical launcher runtime identity is missing"
        );
        if let Some(runtime) = &self.runtime {
            runtime.validate()?;
        }
        ensure!(
            self.original != proxy(),
            "Historical launcher baseline contains a proxy"
        );
        super::super::launcher::validate_entries(&self.mods)?;
        for (path, identity) in &self.originals {
            super::super::launcher::destination(path)?;
            if let Some(identity) = identity {
                identity.validate()?;
            }
        }
        ensure!(
            self.mods
                .iter()
                .flat_map(|entry| &entry.files)
                .all(|file| self.originals.contains_key(&file.destination)),
            "Historical launcher source has no original identity"
        );
        Ok(())
    }

    pub(crate) fn restore(&self, current: &Family) -> Result<Family> {
        self.validate()?;
        current.validate()?;
        ensure!(
            self.runtime
                .as_ref()
                .is_none_or(|runtime| runtime == &proxy()),
            "Historical launcher runtime is incompatible with the current protected runtime"
        );
        ensure!(
            self.original == current.original
                && self
                    .originals
                    .iter()
                    .all(|(path, identity)| current.originals.get(path) == Some(identity)),
            "Historical launcher requires different original inputs"
        );
        let mut desired = current.clone();
        desired.mods = self.mods.clone();
        desired.installed = self.installed;
        ensure!(
            desired.installed == desired.support_required(),
            "This launcher revision is incompatible with current game ownership; keep the required runtime support"
        );
        desired.validate()?;
        Ok(desired)
    }

    pub(crate) fn payloads(&self) -> Result<BTreeMap<String, Content>> {
        self.validate()?;
        let mut result = BTreeMap::new();
        let mut add = |path: String, identity: &Identity| {
            result.insert(
                path,
                Content {
                    size: identity.size,
                    sha256: identity.sha256.clone(),
                },
            );
        };
        add(format!("original/{BINK}"), &self.original);
        for (path, identity) in &self.originals {
            if let Some(identity) = identity {
                add(format!("original/{path}"), identity);
            }
        }
        for entry in &self.mods {
            for source in &entry.sources {
                add(
                    format!("source/{}/{}", entry.source_sha256, source.relative),
                    &Identity {
                        size: source.size,
                        sha256: source.sha256.clone(),
                    },
                );
            }
        }
        if let Some(runtime) = &self.runtime {
            add("runtime/proxy".into(), runtime);
        }
        Ok(result)
    }

    pub(crate) fn source(&self, data: &Path, logical: &str) -> Result<PathBuf> {
        ensure!(
            self.payloads()?.contains_key(logical),
            "Unknown historical launcher payload"
        );
        if let Some(path) = logical.strip_prefix("original/") {
            return Ok(data
                .join("mele-family-originals")
                .join(self.location.to_string())
                .join(path));
        }
        if let Some(path) = logical.strip_prefix("source/") {
            return Ok(data.join("mele-launcher-sources").join(path));
        }
        ensure!(
            logical == "runtime/proxy",
            "Unknown launcher payload anchor"
        );
        Ok(data.join("mele-components/artifacts").join(
            &self
                .runtime
                .as_ref()
                .context("Missing historical runtime")?
                .sha256,
        ))
    }
}

pub(crate) async fn dependency(
    tracker: &Tracker,
    game: &Game,
    desired: &State,
) -> Result<Option<Revision>> {
    let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
    let family = tracker.mele_family(location.id).await?;
    if required(Some(desired)) {
        ensure!(
            family
                .as_ref()
                .is_some_and(|family| family.installed && family.owners.contains(&game.id)),
            "Apply shared launcher runtime support before activating this game"
        );
    }
    let Some(family) = family else {
        return Ok(None);
    };
    let inspected = inspect(tracker, game, desired, false, Control::recovery())
        .await?
        .context("Shared launcher ownership disappeared during inspection")?;
    ensure!(
        inspected.change.previous == family,
        "Shared launcher ownership changed during inspection"
    );
    Ok(Some(Revision::capture(location.id, &family)?))
}

pub(crate) enum Action {
    Mods(Vec<super::super::launcher::Entry>),
    Support(super::super::recipe::Recipe),
    Restore(Revision),
}

pub(crate) struct Prepared {
    pub(crate) change: Change,
    pub(crate) revision: Revision,
    pub(crate) games: BTreeSet<String>,
    root: PathBuf,
    stage: PathBuf,
    data: PathBuf,
    lease: std::sync::Arc<super::super::operation::Lease>,
    discarded: bool,
}

impl Prepared {
    pub(crate) fn source(&self, path: &str) -> PathBuf {
        if self
            .change
            .operations
            .iter()
            .any(|operation| operation.path == path && operation.after.is_some())
        {
            self.stage.join("new").join(path)
        } else {
            self.root.join(path)
        }
    }
    pub(crate) fn payload(&self, path: &str) -> Result<PathBuf> {
        self.revision.source(&self.data, path)
    }
    pub(crate) async fn verify_sources(&self) -> Result<()> {
        let data = self.data.clone();
        let entries = self.change.desired.mods.clone();
        let lease = self.lease.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            sources(&data, &entries, &Control::recovery())
        })
        .await
        .context("Shared source verification stopped")?
    }

    pub(crate) async fn discard(mut self) -> Result<()> {
        self.discarded = true;
        let lease = self.lease.clone();
        let change = self.change.clone();
        let stage = self.stage.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            change.cleanup(&stage)
        })
        .await
        .context("Shared preparation cleanup stopped")?
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if self.discarded {
            return;
        }
        let lease = self.lease.clone();
        let change = self.change.clone();
        let stage = self.stage.clone();
        std::thread::spawn(move || {
            let _lease = lease;
            if let Err(error) = change.cleanup(&stage) {
                eprintln!("Abandoned shared preparation cleanup failed: {error:#}");
            }
        });
    }
}

impl Change {
    pub(crate) fn generation_files(&self) -> BTreeMap<String, (Option<Content>, Option<Content>)> {
        let before = self.previous.files();
        let after = self.desired.files();
        self.paths()
            .into_iter()
            .map(|path| {
                let a = if self.missing.contains(&path) {
                    None
                } else {
                    before.get(&path).map(|id| Content {
                        size: id.size,
                        sha256: id.sha256.clone(),
                    })
                };
                let b = after.get(&path).map(|id| Content {
                    size: id.size,
                    sha256: id.sha256.clone(),
                });
                (path, (a, b))
            })
            .collect()
    }

    pub(crate) fn validate_shared(
        &self,
        initiator: &str,
        deployed: &BTreeMap<String, Option<State>>,
    ) -> Result<()> {
        self.previous.validate()?;
        self.desired.validate()?;
        ensure!(
            self.desired.installed == self.desired.support_required(),
            "Shared application omits required runtime support"
        );
        ensure!(
            self.location_id > 0
                && self.previous.original == self.desired.original
                && self.previous.originals.iter().all(|(path, id)| self
                    .desired
                    .originals
                    .get(path)
                    == Some(id)),
            "Shared restoration must preserve original-file protection"
        );
        ensure!(
            self.operations == self.operations()?,
            "Shared journal operations differ from their recorded configurations"
        );
        ensure!(
            self.previous
                .owners
                .iter()
                .filter(|owner| owner.as_str() != initiator)
                .all(|owner| self.desired.owners.contains(owner)),
            "Shared changes cannot remove another game's ownership"
        );
        for (game, state) in deployed {
            if required(state.as_ref()) {
                ensure!(
                    self.desired.installed && self.desired.owners.contains(game),
                    "Shared restore conflicts with a deployed game's runtime ownership"
                );
            }
        }
        ensure!(
            self.desired
                .owners
                .iter()
                .all(|owner| deployed.contains_key(owner)),
            "Shared ownership refers to a game outside this installation"
        );
        Ok(())
    }

    pub(crate) async fn generation_root(&self, tracker: &Tracker, game: &Game) -> Result<PathBuf> {
        root(tracker, game, self.location_id).await
    }
}

fn sources(
    data: &Path,
    entries: &[super::super::launcher::Entry],
    control: &Control,
) -> Result<()> {
    for entry in entries {
        super::super::launcher::verify_source(
            &super::super::launcher::source_root(data, &entry.source_sha256),
            entry,
            control,
        )?;
    }
    Ok(())
}

pub(crate) async fn prepare(
    tracker: Tracker,
    game: Game,
    action: Action,
    data: PathBuf,
    control: crate::core::generations::content::Control,
) -> Result<Prepared> {
    let control = Control::new(
        control.cancelled,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let lease = super::super::operation::Lease::acquire(&control).await?;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    let plan = match action {
        Action::Mods(entries) => edit(&tracker, &game, entries, &data, control.clone()).await?,
        Action::Support(recipe) => {
            recipe.validate()?;
            ensure!(
                recipe.target.game_id() == game.id,
                "Shared support recipe belongs to another game"
            );
            inspect_recipe(&tracker, &game, &recipe, false, control.clone())
                .await?
                .context("No shared launcher change is required")?
        }
        Action::Restore(revision) => {
            let previous = tracker
                .mele_family(revision.location)
                .await?
                .context("Shared launcher ownership is unavailable")?;
            let desired = revision.restore(&previous)?;
            let root = root(&tracker, &game, revision.location).await?;
            let mut change = Change {
                launcher_edit: true,
                location_id: revision.location,
                previous,
                desired,
                missing: Vec::new(),
                operations: Vec::new(),
            };
            change.operations = change.operations()?;
            Plan {
                root,
                change,
                recorded: true,
            }
        }
    };
    let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
    let games: BTreeSet<_> = location
        .bindings
        .iter()
        .filter(|binding| {
            binding.role == FolderRole::Game && binding.game_id.starts_with("mass-effect-le")
        })
        .map(|binding| binding.game_id.clone())
        .collect();
    let mut deployed = BTreeMap::new();
    for game in &games {
        deployed.insert(game.clone(), tracker.mele_deployment(game).await?);
    }
    plan.change.validate_shared(&game.id, &deployed)?;
    plan.preserve(&tracker, &data, control.clone()).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let stage = plan.change.storage(&data, &id);
    let copy = plan.clone();
    let staging = data.clone();
    let held = lease.clone();
    tokio::task::spawn_blocking(move || {
        let _lease = held;
        sources(&staging, &copy.change.desired.mods, &control)?;
        let result = copy.stage(&staging, &id, &control);
        if result.is_err() {
            copy.change
                .cleanup(&copy.change.storage(&staging, &id))
                .context("Failed shared preparation left staging that requires inspection")?;
        }
        result
    })
    .await
    .context("Shared launcher staging stopped")??;
    Ok(Prepared {
        revision: Revision::capture(plan.change.location_id, &plan.change.desired)?,
        change: plan.change,
        games,
        root: plan.root,
        stage,
        data,
        lease,
        discarded: false,
    })
}

impl Change {
    pub(crate) async fn verify_generation(
        &self,
        tracker: &Tracker,
        game: &Game,
        applied: bool,
    ) -> Result<()> {
        let root = self.generation_root(tracker, game).await?;
        let change = self.clone();
        tokio::task::spawn_blocking(move || change.verify(&root, !applied, &Control::recovery()))
            .await
            .context("Shared launcher file verification stopped")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(owners: &[&str], installed: bool) -> Family {
        Family {
            version: 1,
            original: Identity {
                size: 8,
                sha256: "a".repeat(64),
            },
            owners: owners.iter().map(|id| (*id).into()).collect(),
            installed,
            mods: Vec::new(),
            originals: BTreeMap::new(),
        }
    }

    // @variants: both
    #[tokio::test]
    async fn abandoned_shared_preparation_cleans_staging_before_releasing_its_lease() -> Result<()>
    {
        let temp = tempfile::tempdir()?;
        let stage = temp.path().join("stage");
        fs::create_dir(&stage)?;
        let family = family(&[], false);
        let change = Change {
            location_id: 1,
            launcher_edit: true,
            previous: family.clone(),
            desired: family.clone(),
            missing: Vec::new(),
            operations: Vec::new(),
        };
        let lease = super::super::super::operation::Lease::acquire(&Control::recovery()).await?;
        let prepared = Prepared {
            change,
            revision: Revision::capture(1, &family)?,
            games: BTreeSet::new(),
            root: temp.path().into(),
            stage: stage.clone(),
            data: temp.path().into(),
            lease,
            discarded: false,
        };
        drop(prepared);
        let _next = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            super::super::super::operation::Lease::acquire(&Control::recovery()),
        )
        .await??;
        assert!(!stage.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn historical_launcher_restoration_preserves_current_game_ownership() -> Result<()> {
        let original = Revision::capture(1, &family(&["mass-effect-le1"], true))?;
        let current = family(
            &["mass-effect-le1", "mass-effect-le2", "mass-effect-le3"],
            true,
        );
        let restored = original.restore(&current)?;
        assert_eq!(restored.owners, current.owners);
        assert_eq!(original.id()?, Revision::capture(1, &current)?.id()?);
        let vanilla = Revision::capture(1, &family(&[], false))?;
        assert!(vanilla.restore(&current).is_err());
        assert!(original.restore(&family(&[], false)).is_err());
        let mut changed = current;
        changed.original.sha256 = "b".repeat(64);
        assert!(original.restore(&changed).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn shared_restore_rejects_foreign_owners_and_changed_original_protection() -> Result<()> {
        let previous = family(&["mass-effect-le1", "mass-effect-le2"], true);
        let mut change = Change {
            location_id: 1,
            launcher_edit: true,
            previous: previous.clone(),
            desired: previous,
            missing: Vec::new(),
            operations: Vec::new(),
        };
        let deployed = BTreeMap::from([
            ("mass-effect-le1".into(), None),
            ("mass-effect-le2".into(), None),
        ]);
        change.validate_shared("mass-effect-le1", &deployed)?;
        change.desired.owners.remove("mass-effect-le2");
        assert!(
            change
                .validate_shared("mass-effect-le1", &deployed)
                .is_err()
        );
        change.desired = change.previous.clone();
        change.desired.owners.insert("mass-effect-le3".into());
        assert!(
            change
                .validate_shared("mass-effect-le1", &deployed)
                .is_err()
        );
        change.desired = change.previous.clone();
        change.desired.original.sha256 = "b".repeat(64);
        assert!(
            change
                .validate_shared("mass-effect-le1", &deployed)
                .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
pub(crate) async fn seed_test_proxy(data: &Path) -> Result<()> {
    let selections: Vec<_> = components::required(super::super::Target::Le1)
        .into_iter()
        .filter(|selection| selection.component == Component::BinkProxy)
        .collect();
    components::test_cache::seed(data, &selections).await
}
