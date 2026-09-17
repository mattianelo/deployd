use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::Target;
use super::m3cd::{self, M3cdPlan};
use super::m3da::{self, M3daPlan};
use super::m3m::{self, M3mPlan};
use super::m3za::{self, M3zaPlan};
use super::manifest::{Manifest, join_directory, relative_path};
use super::plot::{self, PlotPlan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Transformation {
    MergeMod,
    EmbeddedTlk,
    Bio2Da,
    ConfigDelta,
    PlotManager,
    SquadmateOutfit,
    Email,
    TextureOverride,
    PrecompiledTextureOverride,
    GlobalShader,
    MemTexture,
}

impl Transformation {
    pub(crate) fn supported(self) -> bool {
        matches!(
            self,
            Self::MergeMod
                | Self::EmbeddedTlk
                | Self::Bio2Da
                | Self::ConfigDelta
                | Self::PlotManager
                | Self::SquadmateOutfit
                | Self::Email
                | Self::PrecompiledTextureOverride
                | Self::GlobalShader
        )
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MergeMod => "M3M package merge",
            Self::EmbeddedTlk => "embedded TLK merge",
            Self::Bio2Da => "M3DA table merge",
            Self::ConfigDelta => "M3CD configuration merge",
            Self::PlotManager => "plot-manager merge",
            Self::SquadmateOutfit => "squadmate-outfit merge",
            Self::Email => "email merge",
            Self::TextureOverride => "M3TO source compilation",
            Self::PrecompiledTextureOverride => "precompiled M3TO texture override",
            Self::GlobalShader => "M3GS shader merge",
            Self::MemTexture => "MEM texture installation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceFile {
    pub(crate) relative: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileMapping {
    pub(crate) source: String,
    pub(crate) destination: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TransformJob {
    pub(crate) kind: Transformation,
    pub(crate) source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackagePlan {
    #[serde(default)]
    pub(crate) images: BTreeMap<String, Vec<u8>>,
    #[serde(default)]
    pub(super) active_tlk: BTreeSet<String>,
    #[serde(default)]
    pub(super) active_options: BTreeSet<String>,
    pub(crate) manifest: Manifest,
    pub(crate) sources: Vec<SourceFile>,
    pub(crate) files: Vec<FileMapping>,
    pub(crate) jobs: Vec<TransformJob>,
    pub(crate) m3da: Vec<M3daPlan>,
    pub(crate) m3cd: Vec<M3cdPlan>,
    pub(crate) m3m: Vec<M3mPlan>,
    pub(super) m3to: Vec<super::m3to::M3toPlan>,
    pub(crate) embedded_tlk: Option<M3zaPlan>,
    pub(crate) plot: Vec<PlotPlan>,
    #[serde(default)]
    pub(super) merge_manifests: Vec<super::merge_manifest::Plan>,
    #[serde(default)]
    pub(super) m3gs: Vec<super::m3gs::Plan>,
    pub(crate) source_sha256: String,
}

impl PackagePlan {
    pub(crate) fn inspect(root: &Path, selected: Option<Target>) -> Result<Self> {
        let mut sources = scan(root)?;
        let manifests = manifest_sources(&sources);
        if manifests.is_empty() {
            super::binary::inspect(root, &sources)?;
            let plan = Self::manual(
                sources,
                selected.context("Select LE1, LE2, or LE3 explicitly for a manual MELE archive")?,
            )?;
            super::binary::inspect_mappings(root, &plan)?;
            return Ok(plan);
        }
        let manifest_source = select_game_manifest(root, &manifests, selected)?.clone();
        if manifests.len() > 1 {
            sources = package_sources(&sources, &manifest_source)?;
        }
        super::binary::inspect(root, &sources)?;
        ensure!(
            manifest_source.size <= 1024 * 1024,
            "moddesc.ini exceeds the 1 MiB limit"
        );
        let manifest_path = &manifest_source.relative;
        let package_dir = manifest_path
            .rsplit_once('/')
            .map(|(parent, _)| format!("{parent}/"))
            .unwrap_or_default();
        ensure!(
            !sources.iter().any(|file| file
                .relative
                .to_ascii_lowercase()
                .ends_with("fomod/moduleconfig.xml")),
            "A package containing both FOMOD and moddesc.ini needs an explicit installer choice"
        );
        let mut text = String::new();
        File::open(root.join(manifest_path))?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut text)
            .context("Cannot read moddesc.ini as UTF-8")?;
        let manifest = Manifest::parse(&text)?;
        ensure!(
            !manifest
                .dlc
                .iter()
                .map(|(_, destination)| destination)
                .chain(&manifest.obsolete_dlc)
                .any(|name| name.eq_ignore_ascii_case(super::merge_dlc::NAME)),
            "Archives cannot install or retire the reserved generated merge DLC"
        );
        ensure!(
            selected.is_none_or(|target| target == manifest.target),
            "This package targets {}; select that game before installing",
            manifest.target.game_id()
        );
        let index: BTreeMap<_, _> = sources
            .iter()
            .map(|file| (file.relative.to_lowercase(), file))
            .collect();
        let resolve = |name: &str| -> Result<&SourceFile> {
            let relative = format!("{package_dir}{}", relative_path(name)?);
            index
                .get(&relative.to_lowercase())
                .copied()
                .with_context(|| format!("Package source '{name}' is missing"))
        };
        let mut files = Vec::new();
        let mut jobs = Vec::new();
        let mut destinations = BTreeSet::new();
        let mut add_file = |source: &SourceFile, destination: String| -> Result<()> {
            let destination = relative_path(&destination)?;
            super::binary::mapping(&FileMapping {
                source: source.relative.clone(),
                destination: destination.clone(),
            })?;
            let folded = destination.to_lowercase();
            ensure!(
                !destinations.iter().any(|existing: &String| folded
                    .starts_with(&format!("{existing}/"))
                    || existing.starts_with(&format!("{folded}/"))),
                "A package destination is both a file and a directory: '{destination}'"
            );
            ensure!(
                destinations.insert(destination.to_lowercase()),
                "Duplicate or case-colliding destination '{destination}'"
            );
            if matches!(
                transformation(&source.relative),
                Some(
                    Transformation::TextureOverride
                        | Transformation::PrecompiledTextureOverride
                        | Transformation::GlobalShader
                )
            ) {
                ensure!(
                    source
                        .relative
                        .rsplit('.')
                        .next()
                        .map(str::to_ascii_lowercase)
                        == destination.rsplit('.').next().map(str::to_ascii_lowercase),
                    "Transformation inputs cannot be renamed to another file format"
                );
            }
            if let Some(kind) =
                transformation(&destination).or_else(|| transformation(&source.relative))
            {
                jobs.push(TransformJob {
                    kind,
                    source: source.relative.clone(),
                });
            }
            files.push(FileMapping {
                source: source.relative.clone(),
                destination,
            });
            Ok(())
        };
        for (source_dir, destination_dir) in &manifest.dlc {
            let prefix = format!("{package_dir}{source_dir}/").to_lowercase();
            let mut found = false;
            for source in &sources {
                if source.relative.to_lowercase().starts_with(&prefix) {
                    found = true;
                    let suffix = source
                        .relative
                        .split('/')
                        .skip(prefix.matches('/').count())
                        .collect::<Vec<_>>()
                        .join("/");
                    add_file(source, format!("DLC/{destination_dir}/{suffix}"))?;
                }
            }
            ensure!(
                found,
                "Custom DLC source '{source_dir}' is missing or empty"
            );
        }
        for (source, destination) in &manifest.basegame {
            if manifest.structured {
                let directory = join_directory(
                    if package_dir.is_empty() {
                        "."
                    } else {
                        package_dir.trim_end_matches('/')
                    },
                    source,
                );
                let prefix = if directory == "." || directory.is_empty() {
                    String::new()
                } else {
                    format!("{directory}/")
                };
                let folded = prefix.to_lowercase();
                let mut found = false;
                for file in &sources {
                    if file.relative.to_lowercase().starts_with(&folded)
                        && structured_extension(&file.relative)
                    {
                        let suffix = file
                            .relative
                            .split('/')
                            .skip(prefix.matches('/').count())
                            .collect::<Vec<_>>()
                            .join("/");
                        add_file(
                            file,
                            basegame_destination(&join_directory(destination, &suffix))?,
                        )?;
                        found = true;
                    }
                }
                ensure!(
                    found,
                    "Structured BASEGAME source '{source}' contains no supported game files"
                );
            } else {
                add_file(resolve(source)?, basegame_destination(destination)?)?;
            }
        }
        let mut m3m = Vec::new();
        for merge in manifest
            .merges
            .iter()
            .chain(manifest.alternates.iter().flat_map(|alt| alt.merge_files()))
        {
            let file = resolve(&format!("MergeMods/{merge}"))?;
            if m3m
                .iter()
                .any(|merge: &M3mPlan| merge.source.relative == file.relative)
            {
                continue;
            }
            m3m.push(m3m::inspect(root, file, manifest.target)?);
            jobs.push(TransformJob {
                kind: Transformation::MergeMod,
                source: file.relative.clone(),
            });
        }
        let embedded_tlk = if manifest.embedded_tlk {
            let file = resolve("GAME1_EMBEDDED_TLK/CombinedTLKMergeData.m3za")?;
            jobs.push(TransformJob {
                kind: Transformation::EmbeddedTlk,
                source: file.relative.clone(),
            });
            Some(m3za::inspect(root, file, manifest.target)?)
        } else {
            None
        };
        let m3gs = super::m3gs::inspect(root, &sources, &files, manifest.target, &manifest.format)?;
        let m3to = super::m3to::inspect(root, &sources, &files, manifest.target, &manifest.format)?;
        let plot = plot::inspect(root, &sources, &files, manifest.target, &manifest.format)?;
        let merge_manifests = super::merge_manifest::inspect(
            root,
            &sources,
            &files,
            manifest.target,
            &manifest.format,
        )?;
        let m3da = m3da::inspect(root, &sources, &files)?;
        ensure!(
            m3da.is_empty() || manifest.target == Target::Le1,
            "M3DA only supports LE1"
        );
        let m3cd = m3cd::inspect(root, &sources, &files)?;
        ensure!(
            m3cd.is_empty() || manifest.target == Target::Le1,
            "M3CD inspection currently supports LE1 only"
        );
        let source_sha256 = tree_digest(&sources);
        super::merge_dlc::archive(&files)?;
        let mut plan = Self {
            images: BTreeMap::new(),
            active_tlk: BTreeSet::new(),
            active_options: BTreeSet::new(),
            manifest,
            sources,
            files,
            jobs,
            m3da,
            m3cd,
            m3m,
            m3to,
            embedded_tlk,
            plot,
            merge_manifests,
            m3gs,
            source_sha256,
        };
        plan.images = super::alternates::images::inspect(root, &plan)?;
        super::alternates::validate(&plan)?;
        super::binary::inspect_mappings(root, &plan)?;
        Ok(plan)
    }

    fn manual(sources: Vec<SourceFile>, target: Target) -> Result<Self> {
        let mut files = Vec::new();
        let mut dlc = BTreeSet::new();
        let mut destinations = BTreeSet::<String>::new();
        for source in &sources {
            let relative = source.relative.as_str();
            if sources
                .iter()
                .any(|file| super::binary::executable(&file.relative))
                && !relative.contains('/')
                && ["readme", "license", "licence"]
                    .iter()
                    .any(|prefix| relative.to_ascii_lowercase().starts_with(prefix))
                && Path::new(relative).extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("txt") || ext.eq_ignore_ascii_case("md")
                })
            {
                continue;
            }
            let destination = if relative.starts_with("Binaries/") {
                super::binary::encode(relative)?
            } else if relative.starts_with("ASI/") {
                super::binary::encode(&format!("Binaries/Win64/{relative}"))?
            } else if !relative.contains('/') && relative.to_ascii_lowercase().ends_with(".asi") {
                super::binary::encode(&format!("Binaries/Win64/ASI/{relative}"))?
            } else if let Some(data) = relative.strip_prefix("BioGame/") {
                data.to_string()
            } else if relative.starts_with("DLC/") || relative.starts_with("CookedPCConsole/") {
                relative.to_string()
            } else if relative.starts_with("DLC_MOD_") {
                format!("DLC/{relative}")
            } else {
                anyhow::bail!(
                    "Manual MELE archives require a recognized content layout, Binaries/Win64, ASI, or a standalone ASI plugin; cannot route '{relative}'"
                );
            };
            super::binary::mapping(&FileMapping {
                source: source.relative.clone(),
                destination: destination.clone(),
            })?;
            let folded = destination.to_lowercase();
            ensure!(
                !destinations.iter().any(|path| path == &folded
                    || path.starts_with(&format!("{folded}/"))
                    || folded.starts_with(&format!("{path}/"))),
                "Manual MELE destinations collide"
            );
            destinations.insert(folded);
            let extension = Path::new(&destination)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            ensure!(
                !matches!(
                    extension.as_str(),
                    "m3da"
                        | "m3cd"
                        | "m3m"
                        | "m3za"
                        | "m3to"
                        | "m3gs"
                        | "mem"
                        | "pmu"
                        | "sqm"
                        | "emm"
                        | "btp"
                        | "btm"
                ),
                "Transformation inputs require a supported moddesc.ini; they cannot be copied as manual content"
            );
            if let Some((name, _)) = destination
                .strip_prefix("DLC/")
                .and_then(|path| path.split_once('/'))
            {
                super::manifest::validate_dlc(name)?;
                dlc.insert(name.to_string());
            }
            files.push(FileMapping {
                source: source.relative.clone(),
                destination,
            });
        }
        ensure!(
            !files.is_empty(),
            "Manual MELE archive contains no content files"
        );
        super::merge_dlc::archive(&files)?;
        let source_sha256 = tree_digest(&sources);
        Ok(Self {
            images: BTreeMap::new(),
            active_tlk: BTreeSet::new(),
            active_options: BTreeSet::new(),
            manifest: Manifest {
                minimum_build: 0,
                localization_target: None,
                metadata: BTreeMap::new(),
                alternates: Vec::new(),
                multilists: BTreeMap::new(),
                base_multilists: BTreeMap::new(),
                target,
                version: "1".into(),
                name: "Manual MELE package".into(),
                author: String::new(),
                description: String::new(),
                website: None,
                format: "manual-1".into(),
                dlc: dlc.into_iter().map(|name| (name.clone(), name)).collect(),
                basegame: Vec::new(),
                structured: false,
                merges: Vec::new(),
                embedded_tlk: false,
                texture_runtime: false,
                required_dlc: Vec::new(),
                incompatible_dlc: Vec::new(),
                obsolete_dlc: Vec::new(),
            },
            sources,
            files,
            jobs: Vec::new(),
            m3da: Vec::new(),
            m3cd: Vec::new(),
            m3m: Vec::new(),
            m3to: Vec::new(),
            embedded_tlk: None,
            plot: Vec::new(),
            merge_manifests: Vec::new(),
            m3gs: Vec::new(),
            source_sha256,
        })
    }

    pub(crate) fn needs_binary_approval(&self) -> bool {
        super::binary::needs_approval(self)
    }

    pub(crate) fn option_keys(&self) -> BTreeSet<String> {
        self.embedded_tlk
            .iter()
            .flat_map(|tlk| {
                tlk.option_keys
                    .iter()
                    .filter(|key| {
                        !self
                            .manifest
                            .alternates
                            .iter()
                            .any(|alt| alt.tlk_key() == Some(key.as_str()))
                    })
                    .cloned()
            })
            .chain(
                self.manifest
                    .alternates
                    .iter()
                    .filter(|alt| alt.manual())
                    .map(|alt| alt.key.clone()),
            )
            .collect()
    }

    pub(crate) fn choice_model(&self) -> Result<super::alternates::choices::Model> {
        super::alternates::choices::Model::new(&self.manifest.alternates, &self.manifest.format)
    }

    pub(crate) fn display_options(&self) -> Vec<&super::alternates::Alternate> {
        let mut options = self
            .manifest
            .alternates
            .iter()
            .filter(|alt| !alt.hidden)
            .collect::<Vec<_>>();
        if !self
            .manifest
            .metadata
            .get("modinfo.sortalternates")
            .is_some_and(|value| value.eq_ignore_ascii_case("false"))
        {
            options.sort_by_key(|alt| (alt.sort_index, alt.name.to_lowercase()));
        }
        options
    }

    pub(crate) fn default_options(&self) -> BTreeSet<String> {
        self.manifest
            .alternates
            .iter()
            .filter(|alt| alt.manual() && alt.default)
            .map(|alt| alt.key.clone())
            .collect()
    }

    #[cfg(test)]
    pub(super) fn resolve(
        &mut self,
        root: &Path,
        selected: &BTreeSet<String>,
        available: &BTreeSet<String>,
    ) -> Result<()> {
        self.resolve_context(
            root,
            selected,
            &super::alternates::Context {
                available,
                sizes: &BTreeMap::new(),
                versions: None,
                options: None,
            },
        )
    }

    pub(super) fn resolve_context(
        &mut self,
        root: &Path,
        selected: &BTreeSet<String>,
        context: &super::alternates::Context<'_>,
    ) -> Result<()> {
        if self.manifest.alternates.is_empty() {
            return Ok(());
        }
        super::alternates::apply_with_context(self, selected, context)?;
        let mut destinations = BTreeSet::new();
        self.jobs.retain(|job| {
            matches!(
                job.kind,
                Transformation::MergeMod | Transformation::EmbeddedTlk
            )
        });
        for file in &self.files {
            super::binary::mapping(file)?;
            let folded = file.destination.to_lowercase();
            ensure!(
                !destinations
                    .iter()
                    .any(|existing: &String| existing == &folded
                        || existing.starts_with(&format!("{folded}/"))
                        || folded.starts_with(&format!("{existing}/"))),
                "Alternate destinations collide"
            );
            destinations.insert(folded);
            if matches!(
                transformation(&file.source),
                Some(
                    Transformation::TextureOverride
                        | Transformation::PrecompiledTextureOverride
                        | Transformation::GlobalShader
                )
            ) {
                ensure!(
                    Path::new(&file.source)
                        .extension()
                        .map(|ext| ext.to_ascii_lowercase())
                        == Path::new(&file.destination)
                            .extension()
                            .map(|ext| ext.to_ascii_lowercase()),
                    "Transformation inputs cannot be renamed to another file format"
                );
            }
            if let Some(kind) =
                transformation(&file.destination).or_else(|| transformation(&file.source))
            {
                self.jobs.push(TransformJob {
                    kind,
                    source: file.source.clone(),
                });
            }
        }
        self.m3gs = super::m3gs::inspect(
            root,
            &self.sources,
            &self.files,
            self.manifest.target,
            &self.manifest.format,
        )?;
        self.m3da = m3da::inspect(root, &self.sources, &self.files)?;
        self.m3cd = m3cd::inspect(root, &self.sources, &self.files)?;
        ensure!(
            (self.m3da.is_empty() && self.m3cd.is_empty()) || self.manifest.target == Target::Le1,
            "M3DA and M3CD require LE1"
        );
        self.plot = plot::inspect(
            root,
            &self.sources,
            &self.files,
            self.manifest.target,
            &self.manifest.format,
        )?;
        self.merge_manifests = super::merge_manifest::inspect(
            root,
            &self.sources,
            &self.files,
            self.manifest.target,
            &self.manifest.format,
        )?;
        super::merge_dlc::archive(&self.files)?;
        self.m3to = super::m3to::inspect(
            root,
            &self.sources,
            &self.files,
            self.manifest.target,
            &self.manifest.format,
        )?;
        ensure!(
            self.required_transformations()
                .iter()
                .all(|kind| kind.supported()),
            "Selected alternate requires an unsupported transformation"
        );
        Ok(())
    }

    pub(crate) fn required_transformations(&self) -> BTreeSet<Transformation> {
        self.jobs.iter().map(|job| job.kind).collect()
    }

    pub(crate) fn verify_sources(&self, root: &Path) -> Result<()> {
        let mut sources = scan(root)?;
        let manifests = manifest_sources(&sources);
        if manifests.len() > 1 && self.manifest.format != "manual-1" {
            let manifest = select_game_manifest(root, &manifests, Some(self.manifest.target))?;
            sources = package_sources(&sources, manifest)?;
        }
        ensure!(
            sources == self.sources,
            "Package contents changed after inspection; inspect the package again before installing"
        );
        Ok(())
    }
}

pub(super) fn basegame_destination(path: &str) -> Result<String> {
    if path.starts_with("Binaries/") {
        return super::binary::encode(path);
    }
    let (anchor, relative) = path
        .split_once('/')
        .context("BASEGAME destinations must be beneath BioGame")?;
    ensure!(
        anchor.eq_ignore_ascii_case("BioGame"),
        "BASEGAME destinations outside BioGame require the root-mod workflow"
    );
    super::journal::destination(&format!("BioGame/{relative}"))?;
    Ok(relative.into())
}

fn structured_extension(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        [
            "u", "upk", "sfm", "pcc", "bin", "tlk", "cnd", "ini", "afc", "tfc", "dlc", "sfar",
            "txt", "bik", "bmp", "usf", "isb", "asi", "dll", "json", "toml", "xml", "cfg",
        ]
        .iter()
        .any(|allowed| extension.eq_ignore_ascii_case(allowed))
    })
}

pub(super) fn transformation(path: &str) -> Option<Transformation> {
    match path.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "m3da" => Some(Transformation::Bio2Da),
        "m3cd" => Some(Transformation::ConfigDelta),
        "pmu" => Some(Transformation::PlotManager),
        "sqm" => Some(Transformation::SquadmateOutfit),
        "emm" => Some(Transformation::Email),
        "m3to" => Some(Transformation::TextureOverride),
        "btp" | "btm" => Some(Transformation::PrecompiledTextureOverride),
        "m3gs" => Some(Transformation::GlobalShader),
        "mem" => Some(Transformation::MemTexture),
        _ => None,
    }
}

pub(super) fn scan(root: &Path) -> Result<Vec<SourceFile>> {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()?.join(root)
    };
    super::baseline::directory(&absolute)?;
    let mut sources = Vec::new();
    let mut paths = BTreeSet::new();
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.context("Cannot enumerate package files")?;
        ensure!(
            entry.file_type().is_file() || entry.file_type().is_dir(),
            "Package contains a link or special file"
        );
        let relative = relative_path(
            entry
                .path()
                .strip_prefix(root)?
                .to_str()
                .context("Package paths must be valid UTF-8")?,
        )?;
        ensure!(
            paths.insert(relative.to_lowercase()),
            "Case-colliding package path '{relative}'"
        );
        if entry.file_type().is_dir() {
            continue;
        }
        super::binary::source_name(&relative)?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        ensure!(
            metadata.is_file() && metadata.nlink() == 1,
            "Package sources must be independent regular files"
        );
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(entry.path())
            .with_context(|| format!("Cannot read package file '{relative}'"))?;
        let before = file.metadata()?;
        let size = before.len();
        ensure!(
            before.is_file()
                && before.nlink() == 1
                && before.dev() == metadata.dev()
                && before.ino() == metadata.ino()
                && size <= i64::MAX as u64
                && sources.len() < 100_000,
            "Package source changed or exceeds inspection limits"
        );
        let mut hash = Sha256::new();
        let copied = std::io::copy(&mut (&file).take(size + 1), &mut hash)?;
        let after = file.metadata()?;
        ensure!(
            copied == size
                && after.len() == size
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec()
                && after.nlink() == 1,
            "Package file '{relative}' changed during inspection"
        );
        sources.push(SourceFile {
            relative,
            size,
            sha256: format!("{:x}", hash.finalize()),
        });
    }
    sources.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(sources)
}

pub(super) fn validate_payload_name(relative: &str) -> Result<()> {
    let extension = Path::new(relative)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        !matches!(
            extension.as_str(),
            "exe" | "dll" | "asi" | "bat" | "cmd" | "ps1" | "sh"
        ),
        "Executable payload '{relative}' requires the binary-mod installation workflow"
    );
    ensure!(
        !matches!(
            extension.as_str(),
            "headmorph" | "me2headmorph" | "me3headmorph" | "ron"
        ),
        "Headmorph installation is deferred; this package must not be deployed as game files"
    );
    Ok(())
}

pub(super) fn tree_digest(files: &[SourceFile]) -> String {
    let mut digest = Sha256::new();
    for file in files {
        digest.update(format!(
            "{}\0{}\0{}\n",
            file.relative, file.size, file.sha256
        ));
    }
    format!("{:x}", digest.finalize())
}

pub(crate) fn discover_manifest(root: &Path) -> Result<Option<PathBuf>> {
    let mut found = None;
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.context("Cannot inspect archive structure")?;
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("moddesc.ini")
        {
            ensure!(
                found.is_none(),
                "Archive contains multiple moddesc.ini files; select a single mod package"
            );
            found = Some(entry.path().to_path_buf());
        }
    }
    Ok(found)
}

pub(crate) fn launcher_manifest(root: &Path) -> Result<Option<PathBuf>> {
    let mut found = None;
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        let entry = entry.context("Cannot inspect archive structure")?;
        if !entry.file_type().is_file()
            || !entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("moddesc.ini")
        {
            continue;
        }
        let metadata = entry.metadata()?;
        ensure!(
            metadata.len() <= 1024 * 1024,
            "moddesc.ini exceeds the 1 MiB limit"
        );
        let mut text = String::new();
        File::open(entry.path())?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut text)
            .context("Cannot read moddesc.ini as UTF-8")?;
        if super::manifest::sections(&text)?
            .get("modinfo")
            .and_then(|values| values.get("game"))
            .is_some_and(|game| game.eq_ignore_ascii_case("LELAUNCHER"))
        {
            ensure!(
                found.is_none(),
                "Archive contains multiple LELAUNCHER moddesc.ini files"
            );
            found = Some(entry.path().to_path_buf());
        }
    }
    Ok(found)
}

#[cfg(test)]
pub(crate) fn contains_launcher_manifest(root: &Path) -> Result<bool> {
    Ok(launcher_manifest(root)?.is_some())
}

pub(super) fn manifest_sources(sources: &[SourceFile]) -> Vec<&SourceFile> {
    sources
        .iter()
        .filter(|source| {
            source
                .relative
                .rsplit('/')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("moddesc.ini"))
        })
        .collect()
}

pub(super) fn manifest_game(root: &Path, source: &SourceFile) -> Result<String> {
    ensure!(
        source.size <= 1024 * 1024,
        "moddesc.ini exceeds the 1 MiB limit"
    );
    let mut text = String::new();
    File::open(root.join(&source.relative))?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .context("Cannot read moddesc.ini as UTF-8")?;
    super::manifest::sections(&text)?
        .get("modinfo")
        .and_then(|values| values.get("game"))
        .filter(|game| !game.trim().is_empty())
        .cloned()
        .context("moddesc.ini is missing [modinfo] game")
}

pub(super) fn package_sources(
    sources: &[SourceFile],
    manifest: &SourceFile,
) -> Result<Vec<SourceFile>> {
    let Some((parent, _)) = manifest.relative.rsplit_once('/') else {
        ensure!(
            manifest_sources(sources).len() == 1,
            "A root moddesc.ini cannot select one package from a multi-package archive"
        );
        return Ok(sources.to_vec());
    };
    let prefix = format!("{parent}/");
    let selected = sources
        .iter()
        .filter(|source| source.relative.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    ensure!(
        manifest_sources(&selected).len() == 1,
        "Selected package contains another moddesc.ini"
    );
    Ok(selected)
}

fn select_game_manifest<'a>(
    root: &Path,
    manifests: &[&'a SourceFile],
    selected: Option<Target>,
) -> Result<&'a SourceFile> {
    if manifests.len() == 1 {
        return Ok(manifests[0]);
    }
    let target = selected
        .context("Archive contains multiple moddesc.ini files; select LE1, LE2, or LE3 first")?;
    let game = match target {
        Target::Le1 => "LE1",
        Target::Le2 => "LE2",
        Target::Le3 => "LE3",
    };
    let mut matches = Vec::new();
    for manifest in manifests {
        if manifest_game(root, manifest)?.eq_ignore_ascii_case(game) {
            matches.push(*manifest);
        }
    }
    ensure!(
        matches.len() == 1,
        "Expected exactly one {} moddesc.ini; found {}",
        game,
        matches.len()
    );
    Ok(matches[0])
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use tempfile::{TempDir, tempdir};

    use super::*;

    // @variants: both
    #[test]
    fn archives_cannot_claim_or_retire_the_generated_merge_dlc() -> Result<()> {
        let root = tempdir()?;
        let directory = root.path().join("dlc_mod_m3_merge/CookedPCConsole");
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("content.pcc"), b"content")?;
        for target in [Target::Le1, Target::Le2, Target::Le3] {
            assert!(PackagePlan::inspect(root.path(), Some(target)).is_err());
        }
        let (root, package) = fixture("")?;
        let manifest = package.join("moddesc.ini");
        let text = fs::read_to_string(&manifest)?;
        for changed in [
            text.replace("destdirs=DLC_MOD_EXAMPLE", "destdirs=dlc_mod_m3_merge"),
            format!("{text}outdatedcustomdlc=DLC_MOD_M3_MERGE\n"),
        ] {
            fs::write(&manifest, changed)?;
            let result = PackagePlan::inspect(root.path(), None);
            assert!(result.is_err());
            assert!(
                format!("{:#}", result.err().context("missing rejection")?).contains("reserved")
            );
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn manual_content_requires_an_explicit_target_and_preserves_game_paths() -> Result<()> {
        for target in [Target::Le1, Target::Le2, Target::Le3] {
            let root = tempdir()?;
            fs::create_dir_all(root.path().join("DLC_MOD_Example/CookedPCConsole"))?;
            fs::write(
                root.path()
                    .join("DLC_MOD_Example/CookedPCConsole/example.pcc"),
                b"content",
            )?;
            assert!(PackagePlan::inspect(root.path(), None).is_err());
            let plan = PackagePlan::inspect(root.path(), Some(target))?;
            assert_eq!(
                plan.files[0].destination,
                "DLC/DLC_MOD_Example/CookedPCConsole/example.pcc"
            );
            assert_eq!(plan.manifest.target, target);
            assert_eq!(plan.manifest.format, "manual-1");
            plan.verify_sources(root.path())?;
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn manual_content_rejects_foreign_anchors_binaries_and_unplanned_transformations() -> Result<()>
    {
        for path in [
            "system/file.2da",
            "Mods/example/file.ws",
            "~docs~/file.txt",
            "Data/example.esp",
            "BioGame/example.dll",
            "BioGame/example.m3da",
            "DLC_MOD_EXAMPLE/PlotManagerUpdate.pmu",
            "BioGame/example.btp",
            "BioGame/example.btm",
            "BioGame/example.headmorph",
        ] {
            let root = tempdir()?;
            let file = root.path().join(path);
            fs::create_dir_all(file.parent().context("missing parent")?)?;
            fs::write(file, b"content")?;
            assert!(
                PackagePlan::inspect(root.path(), Some(Target::Le1)).is_err(),
                "{path}"
            );
        }
        let root = tempdir()?;
        for prefix in ["BioGame/", ""] {
            let path = root
                .path()
                .join(format!("{prefix}CookedPCConsole/example.pcc"));
            fs::create_dir_all(path.parent().context("missing parent")?)?;
            fs::write(path, b"content")?;
        }
        assert!(PackagePlan::inspect(root.path(), Some(Target::Le1)).is_err());
        Ok(())
    }

    fn fixture(wrapper: &str) -> Result<(TempDir, PathBuf)> {
        let root = tempdir()?;
        let package = root.path().join(wrapper);
        fs::create_dir_all(package.join("DLC_MOD_EXAMPLE/CookedPCConsole"))?;
        fs::write(
            package.join("moddesc.ini"),
            "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE1\nmodname=Example\nmoddesc=Example content\nmodver=1.0\nmoddev=Example author\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_EXAMPLE\ndestdirs=DLC_MOD_EXAMPLE\n",
        )?;
        fs::write(
            package.join("DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc"),
            b"example package",
        )?;
        Ok((root, package))
    }

    #[test]
    fn preserves_dlc_ownership_beneath_an_archive_wrapper() -> Result<()> {
        let (root, _) = fixture("Wrapper")?;
        let plan = PackagePlan::inspect(root.path(), Some(Target::Le1))?;
        assert_eq!(
            plan.files,
            vec![FileMapping {
                source: "Wrapper/DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc".into(),
                destination: "DLC/DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc".into(),
            }]
        );
        assert_eq!(plan.sources.len(), 2);
        plan.verify_sources(root.path())?;
        Ok(())
    }

    #[test]
    fn rejects_target_mismatch_and_changed_sources() -> Result<()> {
        let (root, package) = fixture("")?;
        assert!(PackagePlan::inspect(root.path(), Some(Target::Le2)).is_err());
        let plan = PackagePlan::inspect(root.path(), None)?;
        fs::write(
            package.join("DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc"),
            b"changed package",
        )?;
        assert!(plan.verify_sources(root.path()).is_err());
        Ok(())
    }

    #[test]
    fn refuses_ambiguous_manifests_before_wrapper_stripping() -> Result<()> {
        let (root, package) = fixture("Wrapper")?;
        fs::copy(package.join("moddesc.ini"), root.path().join("moddesc.ini"))?;
        assert!(discover_manifest(root.path()).is_err());
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn selects_the_requested_game_from_a_launcher_bundle() -> Result<()> {
        let root = tempdir()?;
        let game = root.path().join("LE2/Game Mod");
        fs::create_dir_all(game.join("DLC_MOD_EXAMPLE/CookedPCConsole"))?;
        fs::write(
            game.join("moddesc.ini"),
            "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE2\nmodname=Game\nmoddesc=Game\nmodver=1\nmoddev=Author\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_EXAMPLE\ndestdirs=DLC_MOD_EXAMPLE\nDLC_MOD_EXAMPLE=Example DLC\n",
        )?;
        fs::write(
            game.join("DLC_MOD_EXAMPLE/CookedPCConsole/Test.pcc"),
            b"game",
        )?;
        let launcher = root.path().join("LELauncher/Launcher Mod");
        fs::create_dir_all(launcher.join("LELAUNCHER"))?;
        fs::write(
            launcher.join("moddesc.ini"),
            "[ModManager]\ncmmver=8\n[ModInfo]\ngame=LELAUNCHER\nmodname=Launcher\n[LELAUNCHER]\nmoddir=LELAUNCHER\n",
        )?;
        fs::write(launcher.join("LELAUNCHER/ME2.bik"), b"launcher")?;

        let plan = PackagePlan::inspect(root.path(), Some(Target::Le2))?;
        assert_eq!(plan.manifest.name, "Game");
        assert_eq!(plan.sources.len(), 2);
        assert!(
            plan.sources
                .iter()
                .all(|source| source.relative.starts_with("LE2/Game Mod/"))
        );
        assert!(contains_launcher_manifest(root.path())?);
        plan.verify_sources(root.path())?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    #[test]
    fn rejects_links_case_collisions_and_executable_payloads() -> Result<()> {
        let (root, package) = fixture("")?;
        let link = package.join("link");
        symlink(package.join("moddesc.ini"), &link)?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::remove_file(link)?;
        fs::write(package.join("MODDESC.INI"), b"collision")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::remove_file(package.join("MODDESC.INI"))?;
        fs::write(package.join("payload.DLL"), b"executable")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    #[test]
    fn discovers_target_merges_inside_dlc_content() -> Result<()> {
        let (root, package) = fixture("")?;
        for filename in ["ConfigDelta-options.m3cd", "PlotManagerUpdate.pmu"] {
            fs::write(
                package
                    .join("DLC_MOD_EXAMPLE/CookedPCConsole")
                    .join(filename),
                if filename.ends_with(".pmu") {
                    "public function bool F9(BioWorldInfo bioWorld, int Argument)\n{ return true; }"
                } else {
                    "[BioUI.ini Engine.UI]\n+Setting=Value"
                },
            )?;
        }
        add_m3da(&package)?;
        let plan = PackagePlan::inspect(root.path(), None)?;
        assert_eq!(plan.m3da.len(), 1);
        assert_eq!(plan.m3cd.len(), 1);
        assert_eq!(plan.plot.len(), 1);
        assert_eq!(plan.plot[0].functions[0].id, 9);
        assert_eq!(plan.m3cd[0].mount, 5);
        assert_eq!(plan.m3cd[0].edits[0].action, '+');
        assert_eq!(plan.m3da[0].mount, 5);
        assert_eq!(plan.m3da[0].merges[0].target, "Engine.pcc");
        assert_eq!(
            plan.m3da[0].merges[0].source,
            "DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc"
        );
        assert_eq!(
            plan.required_transformations(),
            BTreeSet::from([
                Transformation::Bio2Da,
                Transformation::ConfigDelta,
                Transformation::PlotManager
            ])
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_m3cd_without_mount_with_wrong_layout_or_for_other_games() -> Result<()> {
        let (root, package) = fixture("")?;
        let cooked = package.join("DLC_MOD_EXAMPLE/CookedPCConsole");
        let delta = cooked.join("ConfigDelta-options.m3cd");
        fs::write(&delta, "[BioUI.ini Engine.UI]\n+Key=Value")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::write(
            package.join("DLC_MOD_EXAMPLE/AutoLoad.ini"),
            "[ME1DLCMOUNT]\nModMount=5",
        )?;
        let plan = PackagePlan::inspect(root.path(), None)?;
        assert_eq!(plan.m3cd[0].edits[0].key, "Key");
        let manifest = package.join("moddesc.ini");
        let original = fs::read_to_string(&manifest)?;
        for game in ["LE2", "LE3"] {
            fs::write(
                &manifest,
                original.replace("game=LE1", &format!("game={game}")),
            )?;
            assert!(PackagePlan::inspect(root.path(), None).is_err());
        }
        fs::write(&manifest, original)?;
        let wrong = cooked.join("unknown.m3cd");
        fs::rename(&delta, &wrong)?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::create_dir(cooked.join("nested"))?;
        fs::rename(wrong, cooked.join("nested/ConfigDelta-options.m3cd"))?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn orders_config_deltas_and_rejects_changed_operations() -> Result<()> {
        let (root, package) = fixture("")?;
        fs::write(
            package.join("DLC_MOD_EXAMPLE/AutoLoad.ini"),
            "[ME1DLCMOUNT]\nModMount=7",
        )?;
        let cooked = package.join("DLC_MOD_EXAMPLE/CookedPCConsole");
        for name in ["z", "a"] {
            fs::write(
                cooked.join(format!("ConfigDelta-{name}.m3cd")),
                format!("[BioUI.ini Engine.UI]\n>Key={name}"),
            )?;
        }
        let plan = PackagePlan::inspect(root.path(), None)?;
        assert_eq!(plan.m3cd[0].edits[0].value, "a");
        assert_eq!(plan.m3cd[1].edits[0].value, "z");
        plan.verify_sources(root.path())?;
        fs::write(
            cooked.join("ConfigDelta-a.m3cd"),
            "[BioUI.ini Engine.UI]\n?Future=value",
        )?;
        assert!(plan.verify_sources(root.path()).is_err());
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    fn add_m3da(package: &Path) -> Result<()> {
        fs::write(
            package.join("DLC_MOD_EXAMPLE/AutoLoad.ini"),
            "[ME1DLCMOUNT]\nModMount=5",
        )?;
        fs::write(
            package.join("DLC_MOD_EXAMPLE/CookedPCConsole/DLC_MOD_EXAMPLE-tables.m3da"),
            r#"[{"packagefile":"Engine.pcc","mergepackagefile":"Example.pcc","mergetables":["Values_part_1"]},]"#,
        )?;
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_m3da_inputs_and_dlc_overrides_of_target_packages() -> Result<()> {
        let (root, package) = fixture("")?;
        add_m3da(&package)?;
        let cooked = package.join("DLC_MOD_EXAMPLE/CookedPCConsole");
        fs::create_dir(cooked.join("nested"))?;
        fs::write(cooked.join("nested/EXAMPLE.pcc"), b"ambiguous")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::remove_file(cooked.join("nested/EXAMPLE.pcc"))?;
        fs::write(cooked.join("Engine.pcc"), b"override")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::remove_file(cooked.join("Engine.pcc"))?;
        fs::remove_file(package.join("DLC_MOD_EXAMPLE/AutoLoad.ini"))?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    #[test]
    fn rejects_m3da_for_other_legendary_edition_games() -> Result<()> {
        let (root, package) = fixture("")?;
        add_m3da(&package)?;
        let path = package.join("moddesc.ini");
        let original = fs::read_to_string(&path)?;
        for game in ["LE2", "LE3"] {
            fs::write(&path, original.replace("game=LE1", &format!("game={game}")))?;
            assert!(PackagePlan::inspect(root.path(), None).is_err());
        }
        Ok(())
    }

    #[test]
    fn refuses_missing_or_invalid_install_time_merge_inputs() -> Result<()> {
        let (root, package) = fixture("")?;
        let manifest = package.join("moddesc.ini");
        fs::write(
            &manifest,
            fs::read_to_string(&manifest)? + "[BASEGAME]\nmergemods=Example.m3m\n",
        )?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::create_dir(package.join("MergeMods"))?;
        fs::write(package.join("MergeMods/Example.m3m"), b"M3MM\x03invalid")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::write(package.join("MergeMods/Example.m3m"), b"M3MM\x02fixture")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        let text = r#"{"game":"LE1","files":[{"filename":"SFXGame.pcc","changes":[{"entryname":"Example.Fn","scriptupdate":{"scriptfilename":"Example.uc","scripttext":"function Fn() {}"}}]}]}"#;
        let mut data = b"M3MM\x01".to_vec();
        data.extend_from_slice(&(text.len() as i32 + 1).to_le_bytes());
        data.extend_from_slice(text.as_bytes());
        data.push(0);
        data.extend_from_slice(&0i32.to_le_bytes());
        fs::write(package.join("MergeMods/Example.m3m"), data)?;
        assert!(
            PackagePlan::inspect(root.path(), None)?
                .required_transformations()
                .contains(&Transformation::MergeMod)
        );
        Ok(())
    }

    #[test]
    fn rejects_destination_file_directory_collisions_and_disguised_binaries() -> Result<()> {
        let (root, package) = fixture("")?;
        let path = package.join("moddesc.ini");
        let original = fs::read_to_string(&path)?;
        let source = "DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc";
        for destination in [
            "BioGame/a;BioGame/A/file.pcc",
            "BioGame/first.pcc;BioGame/renamed.dll",
        ] {
            fs::write(
                &path,
                format!(
                    "{original}[BASEGAME]\nmoddir=.\nnewfiles={source};{source}\nreplacefiles={destination}\n"
                ),
            )?;
            assert!(PackagePlan::inspect(root.path(), None).is_err());
        }
        Ok(())
    }

    #[test]
    fn protects_official_dlc_and_basegame_destination_scopes() -> Result<()> {
        let (root, package) = fixture("")?;
        let path = package.join("moddesc.ini");
        let original = fs::read_to_string(&path)?;
        for (game, dlc) in [("LE2", "dlc_exp_part01"), ("LE3", "DLC_HEN_PR")] {
            fs::write(
                &path,
                original
                    .replace("game=LE1", &format!("game={game}"))
                    .replace("destdirs=DLC_MOD_EXAMPLE", &format!("destdirs={dlc}")),
            )?;
            assert!(PackagePlan::inspect(root.path(), None).is_err());
        }
        for destination in [
            "BioGame/dlc/DLC_OTHER/file.pcc",
            "Binaries/Win64/file.pcc",
            "../file.pcc",
            "../system/file.pcc",
            "~docs~/file.pcc",
            "Data/file.pcc",
            "Mods/file.pcc",
        ] {
            fs::write(
                &path,
                format!(
                    "{original}[BASEGAME]\nmoddir=.\nnewfiles=DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc\nreplacefiles={destination}\n"
                ),
            )?;
            assert!(PackagePlan::inspect(root.path(), None).is_err());
        }
        fs::write(
            &path,
            format!(
                "{original}[BASEGAME]\nmoddir=.\nnewfiles=DLC_MOD_EXAMPLE/CookedPCConsole/Example.pcc\nreplacefiles=bIoGaMe/CookedPCConsole/Example.pcc\n"
            ),
        )?;
        let plan = PackagePlan::inspect(root.path(), None)?;
        assert!(
            plan.files
                .iter()
                .any(|file| file.destination == "CookedPCConsole/Example.pcc")
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn maps_structured_content_and_official_dlc_without_copying_unrelated_assets() -> Result<()> {
        for wrapper in ["", "Wrapper/"] {
            for game in ["LE1", "LE2", "LE3"] {
                let root = tempfile::tempdir()?;
                let package = root.path().join(wrapper);
                let content = package.join("Payload");
                for path in [
                    "BioGame/CookedPCConsole/Example.PCC",
                    "BioGame/DLC/DLC_MOD_Example/CookedPCConsole/Textures.tfc",
                    "BioGame/DLC/DLC_UPD_Patch01/CookedPCConsole/Official.pcc",
                    "preview.png",
                ] {
                    let file = content.join(path);
                    fs::create_dir_all(file.parent().context("missing parent")?)?;
                    fs::write(file, b"fixture")?;
                }
                fs::write(
                    package.join("moddesc.ini"),
                    format!(
                        "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={game}\nmodname=Structured\nmodver=1.0\nmoddev=Author\nmoddesc=Content\n[BASEGAME]\nmoddir=Payload\ngamedirectorystructure=true\nnewfiles=.\nreplacefiles=.\n"
                    ),
                )?;
                let plan = PackagePlan::inspect(root.path(), None)?;
                assert_eq!(plan.files.len(), 3);
                assert!(
                    plan.files
                        .iter()
                        .any(|file| file.destination == "CookedPCConsole/Example.PCC")
                );
                assert!(
                    plan.files.iter().any(|file| file.destination
                        == "DLC/DLC_UPD_Patch01/CookedPCConsole/Official.pcc")
                );
                assert!(
                    plan.files
                        .iter()
                        .all(|file| file.source.starts_with(&format!("{wrapper}Payload/")))
                );
                assert!(
                    !plan
                        .files
                        .iter()
                        .any(|file| file.source.ends_with("preview.png"))
                );
                fs::write(
                    content.join("BioGame/CookedPCConsole/extra.pcc"),
                    b"changed",
                )?;
                assert!(plan.verify_sources(root.path()).is_err());
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_missing_structured_sources_unsafe_roots_and_overlapping_mappings() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::create_dir_all(root.path().join("Payload/CookedPCConsole"))?;
        fs::write(
            root.path().join("Payload/CookedPCConsole/Example.pcc"),
            b"fixture",
        )?;
        let prefix = "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE2\nmodname=Example\nmodver=1.0\nmoddev=Author\nmoddesc=Content\n[BASEGAME]\nmoddir=Payload\ngamedirectorystructure=true\n";
        for mapping in [
            "newfiles=Missing\nreplacefiles=BioGame",
            "newfiles=.\nreplacefiles=../system",
            "newfiles=.\nreplacefiles=Binaries",
            "newfiles=.;CookedPCConsole\nreplacefiles=BioGame;BioGame/CookedPCConsole",
        ] {
            fs::write(
                root.path().join("moddesc.ini"),
                format!("{prefix}{mapping}\n"),
            )?;
            assert!(
                PackagePlan::inspect(root.path(), None).is_err(),
                "{mapping}"
            );
        }
        fs::write(
            root.path().join("moddesc.ini"),
            format!("{prefix}newfiles=.\nreplacefiles=BioGame\n"),
        )?;
        fs::write(root.path().join("Payload/installer.exe"), b"binary")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        fs::remove_file(root.path().join("Payload/installer.exe"))?;
        fs::remove_file(root.path().join("Payload/CookedPCConsole/Example.pcc"))?;
        fs::write(root.path().join("Payload/preview.png"), b"image")?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    #[ignore = "requires the maintainer-supplied Community Patch 2.0 directory"]
    fn inspects_supplied_community_patch_without_modifying_it() -> Result<()> {
        let root = Path::new("modTesting/LE1 Community Patch");
        let plan = PackagePlan::inspect(root, Some(Target::Le1))?;
        assert_eq!(plan.manifest.version, "2.0");
        assert_eq!(plan.manifest.format, "9.1");
        assert_eq!(plan.m3da.len(), 1);
        assert_eq!(plan.m3da[0].mount, 5);
        assert_eq!(plan.m3da[0].merges[0].tables.len(), 3);
        assert_eq!(plan.m3cd.len(), 2);
        assert_eq!(plan.m3m.len(), 10);
        assert_eq!(
            plan.m3m.iter().filter(|merge| merge.version == 2).count(),
            3
        );
        assert!(
            plan.m3m
                .iter()
                .zip(&plan.manifest.merges)
                .all(|(merge, name)| merge.source.relative == format!("MergeMods/{name}"))
        );
        let localized: Vec<_> = plan
            .m3m
            .iter()
            .flat_map(|plan| &plan.files)
            .filter(|file| file.all_localizations)
            .collect();
        assert_eq!(localized.len(), 2);
        let tlk = plan.embedded_tlk.as_ref().context("Missing TLK plan")?;
        assert_eq!(tlk.version, 2);
        assert_eq!(tlk.updates.len(), 126);
        assert_eq!(
            tlk.updates
                .iter()
                .map(|update| update.strings.len())
                .sum::<usize>(),
            1244
        );
        assert!(tlk.option_keys.is_empty());
        assert_eq!(plan.plot.len(), 1);
        assert_eq!(plan.plot[0].mount, 5);
        assert_eq!(
            plan.plot[0]
                .functions
                .iter()
                .map(|function| function.id)
                .collect::<Vec<_>>(),
            [155, 772, 774, 839, 1138, 1269, 1274, 1374, 1490]
        );
        assert!(
            localized
                .iter()
                .all(|file| file.target_candidates.len() == 13)
        );
        assert!(plan.m3cd.iter().all(|plan| plan.mount == 5));
        assert_eq!(
            plan.m3cd.iter().map(|plan| plan.edits.len()).sum::<usize>(),
            6
        );
        assert_eq!(plan.sources.len(), 1037);
        assert_eq!(
            plan.source_sha256,
            "6515f032e27a1b686e6a1eeb69143cb52cb792ecdf2eb2d8566e67846e6f04d6"
        );
        assert_eq!(
            plan.jobs
                .iter()
                .filter(|job| job.kind == Transformation::MergeMod)
                .count(),
            10
        );
        assert_eq!(
            plan.required_transformations(),
            BTreeSet::from([
                Transformation::MergeMod,
                Transformation::EmbeddedTlk,
                Transformation::Bio2Da,
                Transformation::ConfigDelta,
                Transformation::PlotManager,
            ])
        );
        plan.verify_sources(root)?;
        Ok(())
    }

    // @variants: both
    #[test]
    #[ignore = "requires the maintainer-supplied DLC Timings directory"]
    fn inspects_supplied_dlc_timings_mod() -> Result<()> {
        let root = Path::new("modTesting/DLC Timings Mod");
        let plan = PackagePlan::inspect(root, Some(Target::Le2))?;
        assert_eq!(plan.manifest.name, "DLC Timings Mod");
        assert_eq!(plan.manifest.dlc[0].1, "DLC_MOD_Timings");
        plan.verify_sources(root)?;
        Ok(())
    }

    // @variants: both
    #[test]
    #[ignore = "requires the maintainer-supplied Unofficial LE2 Patch directory"]
    fn inspects_supplied_le2_patch_bundle() -> Result<()> {
        let root = Path::new("modTesting/Unofficial Mass Effect 2 Legendary Edition Patch");
        let plan = PackagePlan::inspect(root, Some(Target::Le2))?;
        assert_eq!(plan.manifest.name, "Unofficial LE2 Patch");
        assert!(contains_launcher_manifest(root)?);
        assert!(
            plan.sources
                .iter()
                .all(|source| source.relative.starts_with("LE2/Unofficial LE2 Patch/"))
        );
        plan.verify_sources(root)?;
        Ok(())
    }
    // @variants: both
    #[test]
    fn outfit_and_email_manifests_use_their_merge_backend() -> Result<()> {
        for (name, kind) in [
            ("SquadmateMergeInfo.sqm", Transformation::SquadmateOutfit),
            ("EmailMergeInfo.emm", Transformation::Email),
        ] {
            let (root, package) = fixture("")?;
            let descriptor = package.join("moddesc.ini");
            fs::write(
                &descriptor,
                fs::read_to_string(&descriptor)?.replace("game=LE1", "game=LE2"),
            )?;
            let content = if kind == Transformation::SquadmateOutfit {
                r#"{"game":"LE2","outfits":[]}"#
            } else {
                r#"{"game":5,"modName":"Example","emails":[]}"#
            };
            fs::write(
                package.join("DLC_MOD_EXAMPLE/CookedPCConsole").join(name),
                content,
            )?;
            let plan = PackagePlan::inspect(root.path(), None)?;
            assert!(plan.required_transformations().contains(&kind));
            assert!(kind.supported());
            fs::remove_file(package.join("moddesc.ini"))?;
            assert!(PackagePlan::inspect(root.path(), Some(Target::Le1)).is_err());
        }
        Ok(())
    }
}
