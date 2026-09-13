use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::super::Target;
use super::super::m3za::StringEdit;

pub(in crate::core::game::mass_effect) const VERSION: &str = "0.11.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileIdentity {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct TlkChange {
    pub(crate) target: String,
    pub(crate) export: String,
    pub(crate) strings: Vec<StringEdit>,
}

#[derive(Clone, Serialize)]
pub(crate) struct PlotContribution {
    pub(crate) dlc: String,
    pub(crate) mount: i32,
    pub(crate) manifest: FileIdentity,
}

#[derive(Clone, Serialize)]
pub(crate) struct ShaderContribution {
    pub(crate) dlc: String,
    pub(crate) mount: i32,
    pub(crate) index: u32,
    pub(crate) shader: FileIdentity,
}

#[derive(Clone, Serialize)]
pub(crate) struct TargetPackage {
    pub(crate) original: FileIdentity,
    pub(crate) current: FileIdentity,
}

#[derive(Clone, Serialize)]
pub(crate) struct Contribution {
    pub(crate) dlc: String,
    pub(crate) mount: i32,
    pub(crate) manifest: FileIdentity,
    pub(crate) packages: Vec<FileIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum M3mKind {
    Asset,
    Class,
    Function,
    Member,
}

#[derive(Clone, Serialize)]
pub(crate) struct M3mOperation {
    pub(crate) target: String,
    pub(crate) entry: String,
    pub(crate) kind: M3mKind,
    pub(crate) input: String,
    pub(crate) source_entry: String,
    pub(crate) allow_new: bool,
}

#[derive(Clone, Serialize)]
pub(crate) struct OutfitMerge {
    pub(crate) dlc: String,
    pub(crate) hench_name: String,
    pub(crate) hench_package: String,
    pub(crate) available_image: String,
    pub(crate) highlight_image: String,
    pub(crate) silhouette_image: Option<String>,
    pub(crate) description_text: i32,
    pub(crate) custom_token: i32,
    pub(crate) plot_flag: i32,
    pub(crate) appearance: i32,
    pub(crate) conditional: i32,
}

#[derive(Clone, Serialize)]
pub(crate) struct EmailMerge {
    pub(crate) dlc: String,
    pub(crate) name: String,
    pub(crate) status: i32,
    pub(crate) trigger: String,
    pub(crate) title: i32,
    pub(crate) description: i32,
    pub(crate) read_transition: Option<i32>,
    pub(crate) in_memory_bool: Option<i32>,
    pub(crate) conditional: i32,
    pub(crate) transition: i32,
}

#[derive(Clone, Serialize)]
pub(crate) struct TextureCopy {
    pub(crate) package: String,
    pub(crate) export: String,
    pub(crate) destination: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct MovieEdit {
    pub(crate) target: FileIdentity,
    pub(crate) movie: FileIdentity,
    pub(crate) images: Vec<TextureCopy>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "operation")]
pub(crate) enum Job {
    #[serde(rename = "mele-m3gs")]
    Shaders {
        game: Target,
        target: TargetPackage,
        contributions: Vec<ShaderContribution>,
    },
    #[serde(rename = "mele-merge-dlc")]
    Dlc {
        game: Target,
        inputs: Vec<FileIdentity>,
        outfits: Vec<OutfitMerge>,
        emails: Vec<EmailMerge>,
        outputs: Vec<String>,
    },
    #[serde(rename = "le2-squad-ui")]
    SquadUi {
        assets: Vec<FileIdentity>,
        movies: Vec<MovieEdit>,
    },
    #[serde(rename = "le1-m3da")]
    Tables {
        targets: Vec<TargetPackage>,
        contributions: Vec<Contribution>,
    },
    #[serde(rename = "le1-m3cd")]
    Config {
        targets: Vec<TargetPackage>,
        contributions: Vec<Contribution>,
    },
    #[serde(rename = "mele-m3m-ordered")]
    M3m {
        game: Target,
        targets: Vec<TargetPackage>,
        dependencies: Vec<FileIdentity>,
        assets: Vec<FileIdentity>,
        scripts: Vec<FileIdentity>,
        jobs: Vec<M3mOperation>,
    },
    #[serde(rename = "le1-tlk")]
    Tlk {
        targets: Vec<FileIdentity>,
        changes: Vec<TlkChange>,
    },
    #[serde(rename = "mele-plot")]
    Plot {
        game: Target,
        target: TargetPackage,
        dependencies: Vec<FileIdentity>,
        contributions: Vec<PlotContribution>,
    },
}

impl Job {
    pub(super) fn game(&self) -> Target {
        match self {
            Self::Shaders { game, .. }
            | Self::M3m { game, .. }
            | Self::Plot { game, .. }
            | Self::Dlc { game, .. } => *game,
            Self::SquadUi { .. } => Target::Le2,
            _ => Target::Le1,
        }
    }

    pub(super) fn operation(&self) -> &'static str {
        match self {
            Self::Shaders { .. } => "mele-m3gs",
            Self::Tables { .. } => "le1-m3da",
            Self::Config { .. } => "le1-m3cd",
            Self::M3m { .. } => "mele-m3m-ordered",
            Self::Tlk { .. } => "le1-tlk",
            Self::Plot { .. } => "mele-plot",
            Self::Dlc { .. } => "mele-merge-dlc",
            Self::SquadUi { .. } => "le2-squad-ui",
        }
    }

    pub(super) fn command(&self) -> &'static str {
        match self {
            Self::Tables { .. } | Self::Config { .. } => "transform",
            Self::Shaders { .. } => "transform-shaders",
            Self::M3m { .. } => "transform-m3m",
            Self::Tlk { .. } => "transform-tlk",
            Self::Plot { .. } => "transform-plot",
            Self::Dlc { .. } => "transform-dlc",
            Self::SquadUi { .. } => "transform-squad-ui",
        }
    }

    pub(super) fn targets(&self) -> Vec<&FileIdentity> {
        match self {
            Self::Tables { targets, .. }
            | Self::Config { targets, .. }
            | Self::M3m { targets, .. } => targets.iter().map(|target| &target.current).collect(),
            Self::Tlk { targets, .. } => targets.iter().collect(),
            Self::Shaders { target, .. } | Self::Plot { target, .. } => vec![&target.current],
            Self::Dlc { .. } => Vec::new(),
            Self::SquadUi { movies, .. } => movies.iter().map(|movie| &movie.target).collect(),
        }
    }

    pub(super) fn outputs(&self) -> Vec<String> {
        match self {
            Self::Dlc { outputs, .. } => outputs.clone(),
            _ => self
                .targets()
                .iter()
                .map(|file| file.path.clone())
                .collect(),
        }
    }

    pub(super) fn originals(&self) -> Vec<&FileIdentity> {
        match self {
            Self::Tlk { .. } | Self::Dlc { .. } | Self::SquadUi { .. } => Vec::new(),
            Self::Shaders { target, .. } | Self::Plot { target, .. } => vec![&target.original],
            Self::Tables { targets, .. }
            | Self::Config { targets, .. }
            | Self::M3m { targets, .. } => targets.iter().map(|target| &target.original).collect(),
        }
    }

    pub(super) fn byte_limit(&self) -> u64 {
        match self {
            Self::Shaders { .. } => 512 * 1024 * 1024,
            Self::Tlk { .. } | Self::Plot { .. } => 2 * 1024 * 1024 * 1024,
            _ => 4 * 1024 * 1024 * 1024,
        }
    }

    pub(super) fn inputs(&self) -> Vec<&FileIdentity> {
        match self {
            Self::Shaders {
                target,
                contributions,
                ..
            } => std::iter::once(&target.current)
                .chain(contributions.iter().map(|item| &item.shader))
                .collect(),
            Self::Dlc { inputs, .. } => inputs.iter().collect(),
            Self::SquadUi { assets, movies } => assets
                .iter()
                .chain(
                    movies
                        .iter()
                        .flat_map(|movie| [&movie.target, &movie.movie]),
                )
                .collect(),
            Self::Tables {
                targets,
                contributions,
            }
            | Self::Config {
                targets,
                contributions,
            } => {
                targets
                    .iter()
                    .map(|target| &target.current)
                    .chain(contributions.iter().flat_map(|item| {
                        std::iter::once(&item.manifest).chain(item.packages.iter())
                    }))
                    .collect()
            }
            Self::M3m {
                targets,
                dependencies,
                assets,
                scripts,
                ..
            } => targets
                .iter()
                .map(|target| &target.current)
                .chain(dependencies.iter())
                .chain(assets.iter())
                .chain(scripts.iter())
                .collect(),
            Self::Tlk { targets, .. } => targets.iter().collect(),
            Self::Plot {
                target,
                dependencies,
                contributions,
                ..
            } => std::iter::once(&target.current)
                .chain(dependencies.iter())
                .chain(contributions.iter().map(|item| &item.manifest))
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capabilities {
    protocol: u32,
    backend: String,
    version: String,
    games: Vec<String>,
    capabilities: Vec<String>,
    validation: Vec<String>,
}

impl Capabilities {
    pub(super) fn validate(&self, operation: &str, game: Target) -> Result<()> {
        ensure!(
            self.protocol == 1 && self.backend == "deployd-mele" && self.version == VERSION,
            "The MELE helper version is incompatible; repair the Deployd package"
        );
        for values in [&self.games, &self.capabilities, &self.validation] {
            ensure!(values.len() <= 64, "Excessive helper capabilities");
            let unique: BTreeSet<_> = values.iter().collect();
            ensure!(
                unique.len() == values.len(),
                "Duplicate helper capabilities"
            );
        }
        ensure!(
            self.games.iter().any(|value| value
                == match game {
                    Target::Le1 => "LE1",
                    Target::Le2 => "LE2",
                    Target::Le3 => "LE3",
                })
                && self.capabilities.iter().any(|value| value == operation),
            "The packaged MELE helper cannot perform {operation}"
        );
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub(super) enum Event {
    #[serde(rename = "progress")]
    Progress {
        protocol: u32,
        completed: u32,
        total: u32,
    },
    #[serde(rename = "complete")]
    Complete {
        protocol: u32,
        outputs: Vec<FileIdentity>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HelperError {
    protocol: u32,
    #[serde(rename = "type")]
    kind: String,
    code: String,
    message: String,
    at: Vec<Option<String>>,
}

impl HelperError {
    pub(super) fn message(&self) -> Result<String> {
        ensure!(
            self.protocol == 1
                && self.kind == "error"
                && self.at.len() <= 128
                && !self.code.is_empty()
                && self.code.len() <= 128
                && !self.code.chars().any(char::is_control)
                && !self.message.is_empty()
                && self.message.len() <= 8192,
            "Invalid MELE helper error response"
        );
        Ok(format!("MELE helper {}: {}", self.code, self.message))
    }
}

#[derive(Default)]
pub(super) struct Transcript {
    progress: Option<(u32, u32)>,
    outputs: Option<Vec<FileIdentity>>,
    expected_total: Option<u32>,
    progress_required: bool,
}

impl Transcript {
    pub(super) fn for_job(job: &Job) -> Self {
        let (expected_total, progress_required) = match job {
            Job::Tables { contributions, .. } | Job::Config { contributions, .. } => {
                (Some(contributions.len() as u32), !contributions.is_empty())
            }
            Job::Shaders { contributions, .. } => {
                (Some(contributions.len() as u32), !contributions.is_empty())
            }
            Job::Dlc { .. } => (Some(7), true),
            Job::SquadUi { movies, .. } => (Some(movies.len() as u32), true),
            Job::M3m { jobs, .. } => (Some(jobs.len() as u32), true),
            Job::Tlk { changes, .. } => (Some(changes.len() as u32), true),
            Job::Plot { contributions, .. } => (
                if contributions.is_empty() {
                    Some(0)
                } else {
                    None
                },
                !contributions.is_empty(),
            ),
        };
        Self {
            expected_total,
            progress_required,
            ..Self::default()
        }
    }

    pub(super) fn accept(&mut self, bytes: &[u8]) -> Result<Option<(u32, u32)>> {
        ensure!(self.outputs.is_none(), "Helper sent data after completion");
        match serde_json::from_slice::<Event>(bytes)? {
            Event::Progress {
                protocol,
                completed,
                total,
            } => {
                ensure!(
                    protocol == 1
                        && total > 0
                        && total <= 4096
                        && completed > 0
                        && completed <= total,
                    "Invalid helper progress"
                );
                ensure!(
                    self.expected_total.is_none_or(|expected| expected == total),
                    "Helper progress does not match the requested work"
                );
                if let Some((previous, previous_total)) = self.progress {
                    ensure!(
                        total == previous_total && completed == previous + 1,
                        "Helper progress changed order or total"
                    );
                } else {
                    ensure!(completed == 1, "Helper progress did not start at one");
                }
                self.progress = Some((completed, total));
                Ok(self.progress)
            }
            Event::Complete { protocol, outputs } => {
                ensure!(
                    protocol == 1 && !outputs.is_empty() && outputs.len() <= 64,
                    "Invalid helper completion"
                );
                ensure!(
                    self.progress.is_none_or(|(done, total)| done == total),
                    "Helper completed before its declared work finished"
                );
                ensure!(
                    !self.progress_required || self.progress.is_some(),
                    "Helper omitted transformation progress"
                );
                self.outputs = Some(outputs);
                Ok(None)
            }
        }
    }

    pub(super) fn finish(self) -> Result<Vec<FileIdentity>> {
        self.outputs
            .ok_or_else(|| anyhow::anyhow!("MELE helper exited without a completion manifest"))
    }
}
