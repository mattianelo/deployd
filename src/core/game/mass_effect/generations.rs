use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::baseline::Baseline;
use super::journal::{self, Identity, State};
use super::package::SourceFile;
use super::recipe::Recipe;
use super::removal::Removals;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub(crate) recipe: Recipe,
    pub(crate) removals: Removals,
    pub(crate) files: Vec<SourceFile>,
    pub(crate) required: Vec<SourceFile>,
}

impl Snapshot {
    pub(crate) fn validate(&self, game_id: &str) -> Result<()> {
        self.recipe.validate()?;
        ensure!(
            self.recipe.target.game_id() == game_id,
            "Historical MELE recipe targets another game"
        );
        self.removals.validate(self.recipe.target)?;
        for inventory in [&self.files, &self.required] {
            ensure!(
                inventory.len() <= 100_000,
                "Historical MELE inventory exceeds its limit"
            );
            let mut seen = BTreeSet::new();
            for file in inventory {
                super::baseline::relative(&file.relative)?;
                super::package::validate_payload_name(&file.relative)?;
                Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                }
                .validate()?;
                ensure!(
                    seen.insert(file.relative.to_lowercase()),
                    "Historical MELE inventory contains colliding paths"
                );
            }
            for path in &seen {
                let mut parent = std::path::Path::new(path).parent();
                while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
                    ensure!(
                        !seen.contains(path.to_str().context("Invalid historical MELE path")?),
                        "Historical MELE path is both a file and directory"
                    );
                    parent = path.parent();
                }
            }
        }
        ensure!(
            self.files.iter().all(|file| !self
                .removals
                .paths
                .iter()
                .any(|path| path.eq_ignore_ascii_case(&file.relative))),
            "Historical MELE file is both installed and removed"
        );
        Ok(())
    }

    pub(crate) fn state(&self, activation: String, profile: String) -> State {
        State {
            version: 4,
            generation: activation,
            profile,
            files: self.files.clone(),
            recipe: Some(self.recipe.clone()),
            removals: self.removals.clone(),
        }
    }

    pub(crate) fn validate_baseline(&self, baseline: &Baseline) -> Result<()> {
        self.validate(&baseline.game_id)?;
        for required in &self.required {
            ensure!(
                baseline
                    .files
                    .iter()
                    .any(|file| file.relative == required.relative
                        && file.sha256 == required.sha256
                        && file.size == required.size),
                "Required vanilla inputs changed; explicitly prepare a new generation against the current game"
            );
        }
        journal::state_files(
            &self.state(uuid::Uuid::nil().to_string(), "historical".into()),
            baseline,
            self.recipe.target,
        )?;
        Ok(())
    }
}

pub(crate) struct Prepared {
    pub(crate) snapshot: Snapshot,
    pub(crate) journal: journal::Journal,
    source: std::path::PathBuf,
    pub(crate) data: std::path::PathBuf,
    _directory: Option<tempfile::TempDir>,
    _lease: std::sync::Arc<super::operation::Lease>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(directory) = self._directory.take() {
            let lease = self._lease.clone();
            let journal = self.journal.clone();
            let data = self.data.clone();
            std::thread::spawn(move || {
                let _lease = lease;
                if let Err(error) = journal.discard_abandoned_generation_stage(&data) {
                    eprintln!("Abandoned MELE preparation cleanup failed: {error:#}");
                }
                drop(directory);
            });
        }
    }
}

impl Prepared {
    pub(crate) fn source(&self, path: &str) -> std::path::PathBuf {
        if self
            .journal
            .generation_targets()
            .iter()
            .any(|(target, _, after)| target == path && after.is_some())
        {
            self.data
                .join("mele-transactions")
                .join(&self.journal.game_id)
                .join(&self.journal.id)
                .join("new")
                .join(path)
        } else {
            self.source.join(path)
        }
    }

    pub(crate) async fn discard(mut self) -> Result<()> {
        let journal = self.journal.clone();
        let data = self.data.clone();
        let directory = self._directory.take();
        let lease = self._lease.clone();
        tokio::spawn(async move {
            let _lease = lease;
            let result = journal.discard_generation_stage(data).await;
            tokio::task::spawn_blocking(move || drop(directory))
                .await
                .context("MELE preparation cleanup worker stopped")?;
            result
        })
        .await
        .context("MELE staging cleanup participant stopped")?
    }
}

pub(crate) async fn prepare(
    tracker: crate::core::tracker::Tracker,
    destination: super::recipe::Destination,
    plan: super::recipe::ValidatedRecipe,
    data: std::path::PathBuf,
    control: crate::core::generations::content::Control,
) -> Result<Prepared> {
    let game = destination.game.clone();
    let engine_control = super::operation::Control::new(
        control.cancelled.clone(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let progress = control.progress.clone();
    let built = super::recipe::prepare_with_control(
        tracker.clone(),
        destination,
        plan,
        data.clone(),
        engine_control.clone(),
        std::sync::Arc::new(move |done, total| progress(done as u64, total as u64)),
        false,
    )
    .await?;
    let mut snapshot = Snapshot {
        recipe: built
            .deployment
            .recipe
            .clone()
            .context("Prepared MELE recipe is missing")?,
        removals: built.deployment.removals.clone(),
        files: built.deployment.files.clone(),
        required: built.required,
    };
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("Missing MELE restoration baseline")?;
    for file in &baseline.files {
        if (snapshot
            .files
            .iter()
            .any(|output| output.relative == file.relative)
            || snapshot.removals.paths.contains(&file.relative))
            && !snapshot
                .required
                .iter()
                .any(|input| input.relative == file.relative)
        {
            snapshot.required.push(SourceFile {
                relative: file.relative.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
            });
        }
    }
    snapshot
        .required
        .sort_by(|a, b| a.relative.cmp(&b.relative));
    snapshot.validate_baseline(&baseline)?;
    let source = built.deployment.source.clone();
    let journal = journal::prepare_with_lease(
        tracker,
        game,
        built.deployment,
        data.clone(),
        engine_control,
        built.lease.clone(),
        false,
    )
    .await?;
    Ok(Prepared {
        snapshot,
        journal,
        source,
        data,
        _directory: Some(built.directory),
        _lease: built.lease,
    })
}

pub(crate) async fn retained(
    tracker: crate::core::tracker::Tracker,
    destination: super::recipe::Destination,
    snapshot: Snapshot,
    source: tempfile::TempDir,
    data: std::path::PathBuf,
    control: crate::core::generations::content::Control,
) -> Result<Prepared> {
    let engine_control = super::operation::Control::new(
        control.cancelled,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let lease = super::operation::Lease::acquire(&engine_control).await?;
    let baseline = tracker
        .load_mele_baseline(&destination.game.id)
        .await?
        .context("Missing MELE restoration baseline")?;
    snapshot.validate_baseline(&baseline)?;
    let deployment = journal::Deployment {
        removals: snapshot.removals.clone(),
        family: None,
        repair_components: destination.repair_components,
        previous: destination.previous,
        profile: destination.profile,
        source: source.path().to_path_buf(),
        files: snapshot.files.clone(),
        recipe: Some(snapshot.recipe.clone()),
    };
    let journal = journal::prepare_with_lease(
        tracker,
        destination.game,
        deployment,
        data.clone(),
        engine_control,
        lease.clone(),
        false,
    )
    .await?;
    Ok(Prepared {
        snapshot,
        journal,
        source: source.path().to_path_buf(),
        data,
        _directory: Some(source),
        _lease: lease,
    })
}

impl Snapshot {
    pub(crate) async fn verify_inputs(
        &self,
        tracker: &crate::core::tracker::Tracker,
        game: &crate::models::game::Game,
        state: Option<&State>,
    ) -> Result<()> {
        let baseline = tracker
            .load_mele_baseline(&game.id)
            .await?
            .context("Missing MELE restoration baseline")?;
        self.validate_baseline(&baseline)?;
        let inputs = self.required.clone();
        let state = state.cloned();
        let root = game.path.clone();
        tokio::task::spawn_blocking(move || {
            let control = super::operation::Control::recovery();
            for input in inputs {
                let expected = if state.as_ref().is_some_and(|state| state.removals.paths.contains(&input.relative)) { None } else {
                    let file = state.as_ref().and_then(|state| state.files.iter().find(|file|file.relative == input.relative)).unwrap_or(&input);
                    Some(Identity {size:file.size,sha256:file.sha256.clone()})
                };
                journal::files::verify(&root,&input.relative,expected.as_ref(),&control).context("Required vanilla inputs changed; explicitly prepare a new generation against the current game")?;
            }
            Ok(())
        }).await.context("MELE vanilla input verification worker stopped")?
    }
}

pub(crate) async fn fresh(
    tracker: crate::core::tracker::Tracker,
    destination: super::recipe::Destination,
    mut recipe: Recipe,
    sources: tempfile::TempDir,
    data: std::path::PathBuf,
    control: crate::core::generations::content::Control,
) -> Result<Prepared> {
    recipe.helper_version = Some(super::helper::protocol::VERSION.into());
    let progress = control.progress.clone();
    let preview = super::recipe::Destination {
        game: destination.game.clone(),
        profile: destination.profile.clone(),
        previous: destination.previous.clone(),
        repair_components: destination.repair_components,
        backend: destination.backend.clone(),
    };
    let plan = super::recipe::inspect_sources(
        tracker.clone(),
        preview,
        recipe,
        (data.clone(), sources.path().into()),
        control.cancelled.clone(),
        std::sync::Arc::new(move |done, total| progress(done as u64, total as u64)),
    )
    .await?;
    let prepared = prepare(tracker, destination, plan, data, control).await?;
    drop(sources);
    Ok(prepared)
}

pub(crate) async fn purge(
    tracker: crate::core::tracker::Tracker,
    game: crate::models::game::Game,
    data: std::path::PathBuf,
    source: tempfile::TempDir,
    control: crate::core::generations::content::Control,
) -> Result<Prepared> {
    let engine_control = super::operation::Control::new(
        control.cancelled,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    );
    let lease = super::operation::Lease::acquire(&engine_control).await?;
    let previous = tracker
        .mele_deployment(&game.id)
        .await?
        .context("No MELE deployment is available to purge")?;
    let target = super::library::target(&game)?;
    let snapshot = Snapshot {
        recipe: Recipe {
            version: 1,
            target,
            backend_version: 1,
            helper_version: None,
            language: "INT".into(),
            packages: Vec::new(),
            components: Vec::new(),
        },
        files: Vec::new(),
        required: Vec::new(),
        removals: Default::default(),
    };
    let deployment = journal::Deployment {
        family: None,
        repair_components: false,
        previous: Some(previous.generation),
        profile: previous.profile,
        source: source.path().into(),
        files: Vec::new(),
        recipe: None,
        removals: Default::default(),
    };
    let journal = journal::prepare_with_lease(
        tracker,
        game,
        deployment,
        data.clone(),
        engine_control,
        lease.clone(),
        false,
    )
    .await?;
    Ok(Prepared {
        snapshot,
        journal,
        source: source.path().into(),
        data,
        _directory: Some(source),
        _lease: lease,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Snapshot, Baseline) {
        let file = SourceFile {
            relative: "BioGame/CookedPCConsole/Engine.pcc".into(),
            size: 8,
            sha256: "a".repeat(64),
        };
        let snapshot = Snapshot {
            recipe: Recipe {
                version: 1,
                target: super::super::Target::Le1,
                backend_version: 1,
                helper_version: None,
                language: "INT".into(),
                packages: Vec::new(),
                components: Vec::new(),
            },
            files: vec![file.clone()],
            required: vec![file.clone()],
            removals: Default::default(),
        };
        let baseline = Baseline {
            game_id: "mass-effect-le1".into(),
            sha256: "b".repeat(64),
            files: vec![super::super::baseline::BaselineFile {
                relative: file.relative,
                size: file.size,
                sha256: file.sha256,
                modified: 0,
            }],
        };
        (snapshot, baseline)
    }

    // @variants: both
    #[test]
    fn historical_outputs_require_the_recorded_vanilla_inputs() -> Result<()> {
        let (snapshot, mut baseline) = fixture();
        snapshot.validate_baseline(&baseline)?;
        baseline.files[0].sha256 = "c".repeat(64);
        assert!(snapshot.validate_baseline(&baseline).is_err());
        baseline.files.clear();
        assert!(snapshot.validate_baseline(&baseline).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn historical_mele_outputs_reject_foreign_anchors_saves_and_unapproved_binaries() -> Result<()>
    {
        let (snapshot, baseline) = fixture();
        for path in [
            "../system/file.pcc",
            "~docs~/file.pcc",
            "BioGame/Saves/save.pcsav",
            "Binaries/Win64/unknown.dll",
            "BioGame/CookedPCConsole/file.dll",
        ] {
            let mut invalid = snapshot.clone();
            invalid.files[0].relative = path.into();
            assert!(invalid.validate_baseline(&baseline).is_err(), "{path}");
        }
        assert!(snapshot.validate("mass-effect-le2").is_err());
        let mut duplicate = snapshot.clone();
        duplicate.files.push(snapshot.files[0].clone());
        assert!(duplicate.validate_baseline(&baseline).is_err());
        Ok(())
    }
}
