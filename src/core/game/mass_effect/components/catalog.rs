use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::super::{Target, package::SourceFile};

pub(super) const BIN: &str = "Binaries/Win64/";
pub(in crate::core::game::mass_effect) const BINK: &str = "Binaries/Win64/bink2w64.dll";
pub(in crate::core::game::mass_effect) const ORIGINAL: &str =
    "Binaries/Win64/bink2w64_original.dll";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Component {
    BinkProxy,
    AutoToc,
    Le1Autoload,
    VisualCpp,
    Le1TextureOverride,
    Le2TextureOverride,
    Le3TextureOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Selection {
    pub(crate) component: Component,
    pub(crate) version: String,
}

pub(super) struct Artifact {
    pub(super) url: &'static str,
    pub(super) size: u64,
    pub(super) sha256: &'static str,
}

pub(super) struct Payload {
    pub(super) path: &'static str,
    pub(super) size: u64,
    pub(super) sha256: &'static str,
}

impl Payload {
    pub(super) fn file(&self) -> SourceFile {
        SourceFile {
            relative: format!("{BIN}{}", self.path),
            size: self.size,
            sha256: self.sha256.into(),
        }
    }
}

impl Component {
    pub(super) fn texture_target(self) -> Option<Target> {
        match self {
            Self::Le1TextureOverride => Some(Target::Le1),
            Self::Le2TextureOverride => Some(Target::Le2),
            Self::Le3TextureOverride => Some(Target::Le3),
            _ => None,
        }
    }

    pub(crate) fn version(self) -> &'static str {
        match self {
            Self::BinkProxy => "2.0.0.14",
            Self::AutoToc => "2",
            Self::Le1Autoload => "13",
            Self::VisualCpp => "14.51.36247.0",
            Self::Le1TextureOverride | Self::Le2TextureOverride | Self::Le3TextureOverride => "3",
        }
    }

    pub(super) fn artifact(self) -> Artifact {
        match self {
            Self::BinkProxy => Artifact {
                url: "https://raw.githubusercontent.com/ME3Tweaks/ME3TweaksCore/b5f006add38dbaea91dde3f16f45ecbc3b0155a9/ME3TweaksCore/GameFilesystem/Bink/64/bink2w64.dll",
                size: 426984,
                sha256: "d493e88ed8bcf726f819ff94a1c07101dbc4c062811bcfaae81b5e089cf78cb3",
            },
            Self::AutoToc => Artifact {
                url: "https://raw.githubusercontent.com/ME3Tweaks/ME3TweaksCore/b5f006add38dbaea91dde3f16f45ecbc3b0155a9/ME3TweaksCore/NativeMods/CachedASI/AutoTOCLE-v2.asi",
                size: 314360,
                sha256: "6d586b9339cdb26868030f2b2a8fbca3e6074f1812c00c7c99e7d967c48874ef",
            },
            Self::Le1Autoload => Artifact {
                url: "https://github.com/ME3Tweaks/LExASIs/releases/download/Autoload-v13/LE1AutoloadEnabler-v13.0.asi",
                size: 468968,
                sha256: "a904658f269b375a602d6e46adf57a2574e6a51ab2f8249606ae1e66c308f3d1",
            },
            Self::Le1TextureOverride => Artifact {
                url: "https://raw.githubusercontent.com/ME3Tweaks/ME3TweaksCore/b5f006add38dbaea91dde3f16f45ecbc3b0155a9/ME3TweaksCore/NativeMods/CachedASI/LE1/LE1TextureOverride-v3.asi",
                size: 453096,
                sha256: "652b8ee169bae7fcbcdbdca71c1ec09e7042d737733d4217d452b77e5eff6db4",
            },
            Self::Le2TextureOverride => Artifact {
                url: "https://raw.githubusercontent.com/ME3Tweaks/ME3TweaksCore/b5f006add38dbaea91dde3f16f45ecbc3b0155a9/ME3TweaksCore/NativeMods/CachedASI/LE2/LE2TextureOverride-v3.asi",
                size: 446952,
                sha256: "735c8fa5cc82c877aca31b4e2a7f8c5a79fa77d330d5dfe9e74fa66a72afba87",
            },
            Self::Le3TextureOverride => Artifact {
                url: "https://raw.githubusercontent.com/ME3Tweaks/ME3TweaksCore/b5f006add38dbaea91dde3f16f45ecbc3b0155a9/ME3TweaksCore/NativeMods/CachedASI/LE3/LE3TextureOverride-v3.asi",
                size: 448488,
                sha256: "0785d455807018f0d13ce588305e05d2ab512dc9be01ff08afb14b12ffaf4284",
            },
            Self::VisualCpp => Artifact {
                url: "https://download.visualstudio.microsoft.com/download/pr/ebdab8e5-1d7b-4d9f-a11b-cbb1720c3b12/843068991DAAA1F73AD9F6239BCE4D0F6A07A51F18C37EA2A867E9BECA71295C/VC_redist.x64.exe",
                size: 18731856,
                sha256: "843068991daaa1f73ad9f6239bce4d0f6a07a51f18c37ea2a867e9beca71295c",
            },
        }
    }

    pub(super) fn payloads(self) -> Vec<Payload> {
        let path = match self {
            Self::BinkProxy => "bink2w64.dll",
            Self::AutoToc => "ASI/AutoTOCLE-v2.asi",
            Self::Le1Autoload => "ASI/LE1AutoloadEnabler-v13.asi",
            Self::Le1TextureOverride => "ASI/LE1TextureOverride-v3.asi",
            Self::Le2TextureOverride => "ASI/LE2TextureOverride-v3.asi",
            Self::Le3TextureOverride => "ASI/LE3TextureOverride-v3.asi",
            Self::VisualCpp => {
                return vec![
                    Payload {
                        path: "msvcp140.dll",
                        size: 643512,
                        sha256: "7c26614e1d733892c2deac7e245ce115504b1d80592dd0a01b08e3e5a55f89ca",
                    },
                    Payload {
                        path: "vcruntime140.dll",
                        size: 178616,
                        sha256: "d1f4225df2cd877dbf130d5668a021dce3f94118455ff5ec952061c30afc9ce7",
                    },
                    Payload {
                        path: "vcruntime140_1.dll",
                        size: 50112,
                        sha256: "a7146c08f89fe5b04541ab507cdb59ff7b44534d4ba3c668a426c6450a03434e",
                    },
                ];
            }
        };
        let artifact = self.artifact();
        vec![Payload {
            path,
            size: artifact.size,
            sha256: artifact.sha256,
        }]
    }
}

pub(crate) fn required(target: Target) -> Vec<Selection> {
    let mut components = vec![Component::BinkProxy, Component::AutoToc];
    if target == Target::Le1 {
        components.extend([Component::Le1Autoload, Component::VisualCpp]);
    }
    components
        .into_iter()
        .map(|component| Selection {
            component,
            version: component.version().into(),
        })
        .collect()
}

pub(super) fn validate(selected: &[Selection], target: Target) -> Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for selection in selected {
        ensure!(
            selection.version == selection.component.version() && seen.insert(selection.component),
            "Unknown or duplicate MELE runtime component version"
        );
        ensure!(
            selection.component != Component::Le1Autoload || target == Target::Le1,
            "Autoload Enabler is only supported for LE1"
        );
    }
    for component in &seen {
        if let Some(game) = component.texture_target() {
            ensure!(
                game == target,
                "Texture Override runtime targets a different LE game"
            );
            ensure!(
                seen.contains(&Component::BinkProxy) && seen.contains(&Component::VisualCpp),
                "Texture Override requires Bink Proxy and the pinned Visual C++ runtime"
            );
        }
    }
    ensure!(
        !seen.contains(&Component::AutoToc) && !seen.contains(&Component::Le1Autoload)
            || seen.contains(&Component::BinkProxy),
        "MELE ASIs require Bink Proxy"
    );
    ensure!(
        !seen.contains(&Component::Le1Autoload) || seen.contains(&Component::VisualCpp),
        "LE1 Autoload requires the pinned Visual C++ runtime"
    );
    Ok(())
}

pub(in crate::core::game::mass_effect) fn texture_runtime(
    selected: &mut Vec<Selection>,
    target: Target,
    enabled: bool,
) {
    selected.retain(|selection| selection.component.texture_target().is_none());
    if enabled {
        let runtime = match target {
            Target::Le1 => Component::Le1TextureOverride,
            Target::Le2 => Component::Le2TextureOverride,
            Target::Le3 => Component::Le3TextureOverride,
        };
        for selection in required(target)
            .into_iter()
            .chain([Component::VisualCpp, runtime].map(|component| Selection {
                component,
                version: component.version().into(),
            }))
        {
            if !selected
                .iter()
                .any(|existing| existing.component == selection.component)
            {
                selected.push(selection);
            }
        }
    }
}

pub(in crate::core::game::mass_effect) fn binary_runtime(
    selected: &mut Vec<Selection>,
    target: Target,
    enabled: bool,
) {
    if !enabled {
        return;
    }
    for selection in required(target).into_iter().chain([Selection {
        component: Component::VisualCpp,
        version: Component::VisualCpp.version().into(),
    }]) {
        if !selected
            .iter()
            .any(|existing| existing.component == selection.component)
        {
            selected.push(selection);
        }
    }
}
