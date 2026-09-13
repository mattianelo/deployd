use super::*;

pub(super) struct Cached {
    pub(super) prepared: Prepared,
    pub(super) inputs: BTreeMap<String, Option<Identity>>,
    game: PathBuf,
    previous: Option<String>,
}

impl Cached {
    pub(super) fn verify_outputs(&self, control: &Control) -> Result<()> {
        let root = self.prepared.directory.path();
        let allowed: BTreeSet<_> = self
            .prepared
            .files
            .iter()
            .map(|file| file.relative.as_str())
            .collect();
        for entry in walkdir::WalkDir::new(root).min_depth(1).follow_links(false) {
            control.check()?;
            let entry = entry.context("Cannot inspect prepared MELE output")?;
            let path = entry
                .path()
                .strip_prefix(root)?
                .to_str()
                .context("Invalid prepared output path")?;
            ensure!(
                entry.file_type().is_dir()
                    || (entry.file_type().is_file() && allowed.contains(path)),
                "Unexpected file or link in prepared MELE output; prepare deployment again"
            );
        }
        for file in &self.prepared.files {
            journal::files::verify(
                root,
                &file.relative,
                Some(&Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                }),
                control,
            )?;
        }
        Ok(())
    }
    pub(super) fn validate(&self, game: &Game, previous: Option<&str>) -> Result<()> {
        ensure!(
            self.game == game.path && self.previous.as_deref() == previous,
            "MELE installation changed after preparation; prepare deployment again"
        );
        Ok(())
    }
}

pub(super) struct Session {
    runtime: tokio::runtime::Handle,
    tracker: Tracker,
    game: Game,
    previous: Option<journal::State>,
    baseline: Baseline,
    inputs: BTreeMap<String, Option<Identity>>,
    data: PathBuf,
    control: Control,
    lease: Arc<Lease>,
    backend: Option<helper::Backend>,
    prepared: Option<Prepared>,
    progress: Progress,
    completed: usize,
    total: usize,
}

impl Session {
    pub(super) fn verify_conditions(
        &mut self,
        plan: &super::super::package::PackagePlan,
    ) -> Result<()> {
        for path in plan
            .manifest
            .alternates
            .iter()
            .flat_map(|alt| alt.required_files())
        {
            let Some(original) = self
                .baseline
                .files
                .iter()
                .find(|file| file.relative.eq_ignore_ascii_case(path))
            else {
                continue;
            };
            let expected = self
                .previous
                .as_ref()
                .and_then(|state| {
                    state
                        .files
                        .iter()
                        .find(|file| file.relative == original.relative)
                })
                .map(|file| Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                })
                .unwrap_or_else(|| Identity {
                    size: original.size,
                    sha256: original.sha256.clone(),
                });
            let expected = if self
                .previous
                .as_ref()
                .is_some_and(|state| state.removals.paths.contains(&original.relative))
            {
                None
            } else {
                Some(expected)
            };
            self.inputs
                .insert(original.relative.clone(), expected.clone());
            journal::files::verify(&self.game.path, &original.relative, expected.as_ref(), &self.control)
                .with_context(|| format!("Conditional input '{}' changed outside Deployd; reconcile the installation before preparing deployment", original.relative))?;
        }
        Ok(())
    }
    pub(super) fn step(
        &mut self,
        step: &installation::Step,
        installation: &installation::Installation,
        packages: &BTreeMap<String, StoredPackage>,
    ) -> Result<Vec<SourceFile>> {
        self.control.check()?;
        let prepared = self
            .prepared
            .take()
            .context("Missing MELE preparation candidate")?;
        let current = prepared
            .files
            .iter()
            .filter(|file| file.relative.starts_with("BioGame/"))
            .map(|file| {
                let input = merges::identity(file)?;
                Ok((input.path.clone(), input))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let source = packages
            .get(&step.package)
            .context("Missing preparation source")?
            .root
            .clone();
        let has_jobs = !step.m3m.is_empty() || !step.tlk.is_empty();
        let prepared = self.runtime.block_on(async {
            if !has_jobs {
                let candidate = pipeline::copy_step(
                    (prepared, current),
                    step.files.clone(),
                    source,
                    self.control.clone(),
                )
                .await?;
                return Ok::<_, anyhow::Error>(
                    pipeline::remove_step(candidate, step.removed.clone(), self.control.clone())
                        .await?
                        .0,
                );
            }
            ensure!(
                prepared.recipe.helper_version.as_deref() == Some(helper::protocol::VERSION),
                "MELE preparation requires the current transformation helper version"
            );
            let originals = &installation.originals;
            let game_inputs: Vec<_> = originals
                .values()
                .map(|file| {
                    let relative = format!("BioGame/{}", file.path);
                    self.previous
                        .as_ref()
                        .and_then(|state| state.files.iter().find(|file| file.relative == relative))
                        .cloned()
                        .unwrap_or_else(|| SourceFile {
                            relative,
                            size: file.size,
                            sha256: file.sha256.clone(),
                        })
                })
                .collect();
            pipeline::verify_game_inputs(
                self.game.path.clone(),
                game_inputs.clone(),
                self.control.clone(),
            )
            .await?;
            let preserved = if originals.is_empty() {
                None
            } else {
                Some(
                    super::super::baseline::originals::preserve_with_lease(
                        self.tracker.clone(),
                        self.game.clone(),
                        originals
                            .keys()
                            .map(|path| format!("BioGame/{path}"))
                            .collect(),
                        self.data.clone(),
                        self.control.clone(),
                        Arc::new(|_, _| {}),
                        self.lease.clone(),
                    )
                    .await?,
                )
            };
            let backend = match &self.backend {
                Some(backend) => backend.clone(),
                None => {
                    let backend = helper::Backend::packaged()?;
                    self.backend = Some(backend.clone());
                    backend
                }
            };
            let report = self.progress.clone();
            let completed = self.completed;
            let total = self.total;
            let result = pipeline::run(
                pipeline::Work {
                    prepared,
                    merges: merges::Merges::originals(originals.clone()),
                    installation: installation::Installation {
                        steps: vec![step.clone()],
                        ..Default::default()
                    },
                    packages: BTreeMap::from([(
                        step.package.clone(),
                        packages
                            .get(&step.package)
                            .context("Missing preparation package")?
                            .clone(),
                    )]),
                    originals: preserved,
                    backend,
                    game: self.game.path.clone(),
                    data: self.data.clone(),
                },
                self.control.clone(),
                Arc::new(move |done, count| {
                    report(completed * 1000 + done * 1000 / count, total * 1000)
                }),
                self.lease.clone(),
            )
            .await?;
            pipeline::verify_game_inputs(self.game.path.clone(), game_inputs, self.control.clone())
                .await?;
            Ok(result)
        })?;
        for (path, identity) in &installation.originals {
            let relative = format!("BioGame/{path}");
            if has_jobs && !prepared.files.iter().any(|file| file.relative == relative) {
                journal::files::verify(
                    prepared.directory.path(),
                    &relative,
                    Some(&Identity {
                        size: identity.size,
                        sha256: identity.sha256.clone(),
                    }),
                    &self.control,
                )?;
                std::fs::remove_file(prepared.directory.path().join(relative))?;
            }
        }
        let files = prepared.files.clone();
        self.prepared = Some(prepared);
        self.completed += 1;
        (self.progress)(self.completed, self.total);
        Ok(files)
    }
}

pub(in crate::core::game::mass_effect) async fn inspect(
    tracker: Tracker,
    destination: Destination,
    recipe: Recipe,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<ValidatedRecipe> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(abandoned.clone());
    let control = Control::new(cancelled, abandoned);
    let lease = Lease::acquire(&control).await?;
    tracker.ensure_location_ready(&destination.game.id).await?;
    tracker.ensure_no_mele_journal(&destination.game.id).await?;
    let previous = tracker.mele_deployment(&destination.game.id).await?;
    ensure!(
        previous.as_ref().map(|state| &state.generation) == destination.previous.as_ref(),
        "MELE deployment changed; prepare deployment again"
    );
    let baseline = tracker
        .load_mele_baseline(&destination.game.id)
        .await?
        .context("MELE restoration baseline is unavailable")?;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        super::super::removal::verify(
            &destination.game.path,
            &baseline,
            previous.as_ref(),
            &BTreeSet::new(),
            &control,
        )?;
        let parent = data.join("mele-rebuilds");
        journal::files::create_directory(&parent)?;
        let prepared = Prepared {
            directory: tempfile::Builder::new()
                .prefix("preparation-")
                .tempdir_in(parent)?,
            removals: Default::default(),
            files: Vec::new(),
            recipe: recipe.clone(),
        };
        let mut session = Session {
            runtime,
            tracker,
            game: destination.game.clone(),
            previous,
            baseline: baseline.clone(),
            inputs: BTreeMap::new(),
            data: data.clone(),
            control: control.clone(),
            lease,
            backend: destination.backend,
            prepared: Some(prepared),
            progress,
            completed: 0,
            total: recipe
                .packages
                .iter()
                .filter(|package| package.enabled)
                .count()
                .max(1),
        };
        let mut plan = validate(recipe, baseline, data, &control, Some(&mut session))?;
        let mut prepared = session
            .prepared
            .take()
            .context("Missing completed MELE preparation")?;
        prepared.recipe = plan.recipe.clone();
        prepared.removals = plan.installation.removals.clone();
        for stored in plan.packages.values() {
            sources::verify(stored, &control)?;
        }
        control.check()?;
        plan.prepared = Some(Cached {
            inputs: session.inputs,
            prepared,
            game: destination.game.path,
            previous: destination.previous,
        });
        Ok(plan)
    })
    .await
    .context("MELE deployment preparation worker failed")?
}

pub(super) async fn verify_inputs(
    root: PathBuf,
    inputs: BTreeMap<String, Option<Identity>>,
    control: Control,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        for (path, identity) in inputs {
            journal::files::verify(&root, &path, identity.as_ref(), &control)
                .with_context(|| format!("Conditional input '{path}' changed after preparation; reconcile the installation and prepare again"))?;
        }
        control.check()
    }).await.context("MELE conditional-input verification failed")?
}
