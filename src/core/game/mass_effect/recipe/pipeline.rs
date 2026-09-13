use super::super::baseline::originals::Preserved;
use super::super::helper::protocol::TargetPackage;
use super::*;

pub(super) const PROGRESS_TOTAL: usize = 5000;
pub(super) type Candidate = (Prepared, BTreeMap<String, helper::FileIdentity>);

pub(super) struct Work {
    pub(super) prepared: Prepared,
    pub(super) merges: merges::Merges,
    pub(super) installation: installation::Installation,
    pub(super) packages: BTreeMap<String, StoredPackage>,
    pub(super) originals: Option<Preserved>,
    pub(super) backend: helper::Backend,
    pub(super) game: PathBuf,
    pub(super) data: PathBuf,
}

pub(super) async fn verify_game_inputs(
    root: PathBuf,
    files: Vec<SourceFile>,
    control: Control,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    tokio::task::spawn_blocking(move || {
        for file in files {
            journal::files::verify(&root, &file.relative,
                Some(&Identity { size: file.size, sha256: file.sha256 }), &control)
                .with_context(|| format!("MELE merge input '{}' changed outside Deployd; reconcile the installation before rebuilding", file.relative))?;
        }
        Ok::<_, anyhow::Error>(())
    }).await.context("MELE game-input verification failed")?
}

pub(super) async fn run(
    work: Work,
    control: Control,
    progress: Progress,
    lease: Arc<Lease>,
) -> Result<Prepared> {
    let (work, mut current) = {
        let control = control.clone();
        tokio::task::spawn_blocking(move || {
            let root = work.prepared.directory.path();
            let mut current = BTreeMap::new();
            for file in work
                .prepared
                .files
                .iter()
                .filter(|file| file.relative.starts_with("BioGame/"))
            {
                let input = merges::identity(file)?;
                current.insert(input.path.clone(), input);
            }
            for (path, original) in &work.merges.originals {
                if !current.contains_key(path) {
                    let relative = format!("BioGame/{path}");
                    journal::files::copy(
                        &work
                            .originals
                            .as_ref()
                            .context("Missing preserved MELE inputs")?
                            .root,
                        &relative,
                        root,
                        &relative,
                        &Identity {
                            size: original.size,
                            sha256: original.sha256.clone(),
                        },
                        &control,
                    )?;
                    current.insert(path.clone(), original.clone());
                }
            }
            for directory in ["mele-transformations", "mele-preparations"] {
                journal::files::create_directory(&work.data.join(directory))?;
            }
            Ok::<_, anyhow::Error>((work, current))
        })
        .await
        .context("MELE merge candidate preparation failed")??
    };
    let mut prepared = work.prepared;
    if !work.installation.steps.is_empty() {
        let total = work.installation.steps.len();
        for (index, step) in work.installation.steps.into_iter().enumerate() {
            let source = work
                .packages
                .get(&step.package)
                .context("Missing installation source")?
                .root
                .clone();
            (prepared, current) = copy_step(
                (prepared, current),
                step.files,
                source.clone(),
                control.clone(),
            )
            .await?;
            progress((index * 1000 + 100) / total, PROGRESS_TOTAL);
            if !step.m3m.is_empty() {
                let originals = work
                    .originals
                    .as_ref()
                    .context("M3M requires preserved originals")?
                    .root
                    .join("BioGame");
                let packages = step
                    .m3m_inputs
                    .iter()
                    .map(|path| {
                        Ok(TargetPackage {
                            original: work
                                .merges
                                .originals
                                .get(path)
                                .context("Missing M3M original")?
                                .clone(),
                            current: current.get(path).context("Missing M3M candidate")?.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let inputs = helper::m3m::prepare_with_lease(
                    helper::m3m::Sources {
                        package: source,
                        original: originals.clone(),
                        candidate: prepared.directory.path().join("BioGame"),
                        packages,
                        plans: step.m3m,
                    },
                    work.data.join("mele-preparations"),
                    control.clone(),
                    lease.clone(),
                )
                .await?;
                let progress = progress.clone();
                let output = helper::transform_with_lease(
                    work.backend.clone(),
                    helper::Inputs {
                        game: work.game.clone(),
                        candidate: inputs.root().to_path_buf(),
                        original: Some(originals),
                    },
                    work.data.join("mele-transformations"),
                    inputs.job.clone(),
                    control.clone(),
                    Arc::new(move |done, count| {
                        progress(
                            (index * 1000 + 100 + done as usize * 400 / count as usize) / total,
                            PROGRESS_TOTAL,
                        )
                    }),
                    lease.clone(),
                )
                .await?;
                (prepared, current) = accept((prepared, current), output, control.clone()).await?;
            }
            let jobs = installation::tlk_jobs(&step.tlk, &current)?;
            let batches = jobs.len();
            for (batch, job) in jobs.into_iter().enumerate() {
                let progress = progress.clone();
                let output = helper::transform_with_lease(
                    work.backend.clone(),
                    helper::Inputs {
                        game: work.game.clone(),
                        candidate: prepared.directory.path().join("BioGame"),
                        original: None,
                    },
                    work.data.join("mele-transformations"),
                    job,
                    control.clone(),
                    Arc::new(move |done, count| {
                        let fraction =
                            (batch * 500 + done as usize * 500 / count as usize) / batches;
                        progress((index * 1000 + 500 + fraction) / total, PROGRESS_TOTAL);
                    }),
                    lease.clone(),
                )
                .await?;
                (prepared, current) = accept((prepared, current), output, control.clone()).await?;
            }
            if !step.removed.is_empty() {
                (prepared, current) =
                    remove_step((prepared, current), step.removed, control.clone()).await?;
            }
            control.check()?;
            progress((index + 1) * 1000 / total, PROGRESS_TOTAL);
        }
    }
    for phase in 0..5 {
        control.check()?;
        let Some(job) = work.merges.job(phase, &current)? else {
            continue;
        };
        let merge_progress = progress.clone();
        let mut output = helper::transform_with_lease(
            work.backend.clone(),
            helper::Inputs {
                game: work.game.clone(),
                original: Some(
                    work.originals
                        .as_ref()
                        .context("Target merges require originals")?
                        .root
                        .join("BioGame"),
                ),
                candidate: prepared.directory.path().join("BioGame"),
            },
            work.data.join("mele-transformations"),
            job,
            control.clone(),
            Arc::new(move |done, total| {
                merge_progress(
                    1000 + phase * 640 + done as usize * 640 / total as usize,
                    PROGRESS_TOTAL,
                )
            }),
            lease.clone(),
        )
        .await?;
        let ui = if phase == 4
            && output
                .files
                .iter()
                .any(|file| file.path.starts_with(".merge-ui/"))
        {
            let inputs = super::squad_ui::prepare(
                super::squad_ui::Sources {
                    candidate: prepared.directory.path().join("BioGame"),
                    generated: output.root(),
                    outputs: output.files.clone(),
                    current: current.clone(),
                    outfits: work
                        .merges
                        .dlc
                        .as_ref()
                        .context("Missing squad UI recipe")?
                        .outfits
                        .clone(),
                },
                work.data.join("mele-preparations"),
                control.clone(),
                lease.clone(),
            )
            .await?;
            let ui_progress = progress.clone();
            Some(
                helper::transform_with_lease(
                    work.backend.clone(),
                    helper::Inputs {
                        game: work.game.clone(),
                        original: None,
                        candidate: inputs.root(),
                    },
                    work.data.join("mele-transformations"),
                    inputs.job.clone(),
                    control.clone(),
                    Arc::new(move |done, total| {
                        ui_progress(4200 + done as usize * 800 / total as usize, PROGRESS_TOTAL)
                    }),
                    lease.clone(),
                )
                .await?,
            )
        } else {
            None
        };
        output.files.retain(|file| {
            !file.path.starts_with(".merge-ui/")
                && ui
                    .as_ref()
                    .is_none_or(|ui| !ui.files.iter().any(|updated| updated.path == file.path))
        });
        (prepared, current) = accept((prepared, current), output, control.clone()).await?;
        if let Some(ui) = ui {
            (prepared, current) = accept((prepared, current), ui, control.clone()).await?;
        }
    }
    let worker_control = control.clone();
    tokio::task::spawn_blocking(move || {
        for stored in work.packages.values() {
            sources::verify(stored, &worker_control)?;
        }
        worker_control.check()
    })
    .await
    .context("MELE source reverification failed")??;
    control.check()?;
    Ok(prepared)
}

pub(super) async fn copy_step(
    candidate: Candidate,
    files: Vec<PlannedFile>,
    source: PathBuf,
    control: Control,
) -> Result<Candidate> {
    tokio::task::spawn_blocking(move || {
        let (mut prepared, mut current) = candidate;
        let mut managed: BTreeMap<_, _> = prepared
            .files
            .into_iter()
            .map(|file| (file.relative.clone(), file))
            .collect();
        for file in files {
            let input = if file.destination.relative.starts_with("BioGame/") {
                let input = merges::identity(&file.destination)?;
                remove_candidate(
                    prepared.directory.path(),
                    &input.path,
                    current.get(&input.path),
                    &control,
                )?;
                Some(input)
            } else {
                super::super::binary::destination(&file.destination.relative)?;
                let before = managed.get(&file.destination.relative);
                journal::files::verify(
                    prepared.directory.path(),
                    &file.destination.relative,
                    before
                        .map(|file| Identity {
                            size: file.size,
                            sha256: file.sha256.clone(),
                        })
                        .as_ref(),
                    &control,
                )?;
                if before.is_some() {
                    std::fs::remove_file(
                        prepared.directory.path().join(&file.destination.relative),
                    )?;
                }
                None
            };
            journal::files::copy(
                &source,
                &file.source,
                prepared.directory.path(),
                &file.destination.relative,
                &Identity {
                    size: file.destination.size,
                    sha256: file.destination.sha256.clone(),
                },
                &control,
            )?;
            if let Some(input) = input {
                current.insert(input.path.clone(), input);
            }
            managed.insert(file.destination.relative.clone(), file.destination);
        }
        prepared.files = managed.into_values().collect();
        Ok((prepared, current))
    })
    .await
    .context("MELE installation file staging failed")?
}

pub(super) async fn remove_step(
    candidate: Candidate,
    paths: Vec<String>,
    control: Control,
) -> Result<Candidate> {
    tokio::task::spawn_blocking(move || {
        let (mut prepared, mut current) = candidate;
        for relative in paths {
            let path = relative
                .strip_prefix("BioGame/")
                .context("Removal outside BioGame")?;
            remove_candidate(prepared.directory.path(), path, current.get(path), &control)?;
            current.remove(path);
            prepared.files.retain(|file| file.relative != relative);
        }
        Ok((prepared, current))
    })
    .await
    .context("MELE obsolete DLC staging failed")?
}

async fn accept(
    candidate: Candidate,
    output: helper::ValidatedOutput,
    control: Control,
) -> Result<Candidate> {
    tokio::task::spawn_blocking(move || {
        let (mut prepared, mut current) = candidate;
        let mut files: BTreeMap<_, _> = prepared
            .files
            .into_iter()
            .map(|file| (file.relative.clone(), file))
            .collect();
        for file in &output.files {
            control.check()?;
            let before = current.get(&file.path);
            ensure!(
                before.is_some() || super::super::merge_dlc::contains(&file.path),
                "Unexpected MELE merge output"
            );
            let relative = format!("BioGame/{}", file.path);
            let root = prepared.directory.path();
            remove_candidate(root, &file.path, before, &control)?;
            journal::files::copy(
                &output.root(),
                &file.path,
                root,
                &relative,
                &Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
                &control,
            )?;
            current.insert(file.path.clone(), file.clone());
            files.insert(
                relative.clone(),
                SourceFile {
                    relative,
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
            );
        }
        prepared.files = files.into_values().collect();
        Ok((prepared, current))
    })
    .await
    .context("MELE merge output staging failed")?
}

fn remove_candidate(
    root: &std::path::Path,
    path: &str,
    before: Option<&helper::FileIdentity>,
    control: &Control,
) -> Result<()> {
    let relative = format!("BioGame/{path}");
    journal::files::verify(
        root,
        &relative,
        before
            .map(|file| Identity {
                size: file.size,
                sha256: file.sha256.clone(),
            })
            .as_ref(),
        control,
    )?;
    if before.is_some() {
        std::fs::remove_file(root.join(relative))?;
    }
    Ok(())
}
