use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use super::super::helper::protocol::{MovieEdit, OutfitMerge, TextureCopy};
use super::super::helper::{FileIdentity, Job};
use super::super::{merge_dlc, squad_movie};
use super::{Control, Identity, Lease, journal};

pub(super) struct Sources {
    pub(super) candidate: PathBuf,
    pub(super) generated: PathBuf,
    pub(super) outputs: Vec<FileIdentity>,
    pub(super) current: BTreeMap<String, FileIdentity>,
    pub(super) outfits: Vec<OutfitMerge>,
}

pub(super) struct Prepared {
    directory: tempfile::TempDir,
    pub(super) job: Job,
    _lease: Arc<Lease>,
}

impl Prepared {
    pub(super) fn root(&self) -> PathBuf {
        self.directory.path().to_path_buf()
    }
}

pub(super) async fn prepare(
    sources: Sources,
    parent: PathBuf,
    control: Control,
    lease: Arc<Lease>,
) -> Result<Prepared> {
    tokio::task::spawn_blocking(move || {
        control.check()?;
        let directory = tempfile::Builder::new()
            .prefix("mele-squad-ui-")
            .tempdir_in(parent)?;
        let root = directory.path();
        let mut assets = BTreeMap::new();
        let mut slots = Vec::new();
        for outfit in &sources.outfits {
            let name = format!(
                "DLC/{}/CookedPCConsole/SFXHenchImages_{}.pcc",
                outfit.dlc, outfit.dlc
            );
            if !assets.contains_key(&name) {
                let input = sources
                    .current
                    .values()
                    .find(|input| input.path.eq_ignore_ascii_case(&name))
                    .context("Missing squadmate image package")?;
                journal::files::copy(
                    &sources.candidate,
                    &input.path,
                    root,
                    &name,
                    &Identity {
                        size: input.size,
                        sha256: input.sha256.clone(),
                    },
                    &control,
                )?;
                assets.insert(
                    name.clone(),
                    FileIdentity {
                        path: name.clone(),
                        ..input.clone()
                    },
                );
            }
            let member = [
                "Vixen",
                "Garrus",
                "Mystic",
                "Grunt",
                "Leading",
                "Tali",
                "Convict",
                "Geth",
                "Thief",
                "Assassin",
                "Professor",
                "Veteran",
            ]
            .iter()
            .position(|name| *name == outfit.hench_name)
            .context("Unknown LE2 squadmate")?;
            slots.push(squad_movie::Slot {
                member,
                appearance: usize::try_from(outfit.appearance)?,
                available: format!("{name}#{}", outfit.available_image),
                highlight: format!("{name}#{}", outfit.highlight_image),
            });
        }
        let mut movies = Vec::new();
        fs::create_dir(root.join(".merge-ui"))?;
        for name in merge_dlc::UI_PACKAGES {
            control.check()?;
            let target = sources
                .outputs
                .iter()
                .find(|file| file.path == format!("{}{name}", merge_dlc::COOKED))
                .context("Missing generated squad UI package")?
                .clone();
            journal::files::copy(
                &sources.generated,
                &target.path,
                root,
                &target.path,
                &Identity {
                    size: target.size,
                    sha256: target.sha256.clone(),
                },
                &control,
            )?;
            let raw = sources
                .outputs
                .iter()
                .find(|file| file.path == format!(".merge-ui/{name}.gfx"))
                .context("Missing game-owned squad UI movie")?;
            journal::files::verify(
                &sources.generated,
                &raw.path,
                Some(&Identity {
                    size: raw.size,
                    sha256: raw.sha256.clone(),
                }),
                &control,
            )?;
            ensure!(
                raw.size <= 16 * 1024 * 1024,
                "Squad UI movie exceeds its size limit"
            );
            let bytes = fs::read(sources.generated.join(&raw.path))?;
            ensure!(
                bytes.len() as u64 == raw.size
                    && format!("{:x}", Sha256::digest(&bytes)) == raw.sha256,
                "Squad UI input changed during preparation"
            );
            let output = squad_movie::extend(&bytes, &slots)?;
            control.check()?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join(&raw.path))?;
            file.write_all(&output.movie)?;
            file.sync_all()?;
            let movie = FileIdentity {
                path: raw.path.clone(),
                size: output.movie.len() as u64,
                sha256: format!("{:x}", Sha256::digest(&output.movie)),
            };
            let images = output
                .images
                .into_iter()
                .map(|image| {
                    let (package, export) = image
                        .source
                        .split_once('#')
                        .context("Invalid squad UI source identity")?;
                    Ok(TextureCopy {
                        package: package.into(),
                        export: export.into(),
                        destination: image.destination,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            movies.push(MovieEdit {
                target,
                movie,
                images,
            });
        }
        let job = Job::SquadUi {
            assets: assets.into_values().collect(),
            movies,
        };
        job.validate()?;
        Ok(Prepared {
            directory,
            job,
            _lease: lease,
        })
    })
    .await
    .context("Squad UI preparation failed")?
}
