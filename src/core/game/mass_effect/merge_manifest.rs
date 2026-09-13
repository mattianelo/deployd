use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::Target;
use super::m3za::read_input;
use super::package::{FileMapping, SourceFile};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Outfit {
    pub(super) henchname: String,
    pub(super) henchpackage: String,
    #[serde(default)]
    pub(super) highlightimage: String,
    pub(super) availableimage: String,
    #[serde(default)]
    pub(super) silhouetteimage: Option<String>,
    #[serde(default)]
    pub(super) descriptiontext0: i32,
    #[serde(default)]
    pub(super) customtoken0: i32,
    #[serde(default = "unconditional")]
    pub(super) plotflag: i32,
}

fn unconditional() -> i32 {
    -1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Email {
    #[serde(default)]
    pub(super) email_name: String,
    pub(super) status_plot_int: i32,
    #[serde(default)]
    pub(super) trigger_conditional: String,
    pub(super) title_str_ref: i32,
    pub(super) desc_str_ref: i32,
    #[serde(default)]
    pub(super) read_transition: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum Game {
    Name(String),
    Number(i32),
}

impl Game {
    fn target(&self) -> Result<Target> {
        match self {
            Self::Name(name) => Target::parse(name),
            Self::Number(4) => Ok(Target::Le1),
            Self::Number(5) => Ok(Target::Le2),
            Self::Number(6) => Ok(Target::Le3),
            _ => anyhow::bail!("Merge manifest requires a Legendary Edition game"),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SquadManifest {
    game: Game,
    outfits: Vec<Outfit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EmailManifest {
    game: Game,
    #[serde(default)]
    mod_name: String,
    #[serde(default)]
    in_memory_bool: Option<i32>,
    emails: Vec<Email>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Content {
    Outfits(Vec<Outfit>),
    Emails {
        mod_name: String,
        in_memory_bool: Option<i32>,
        emails: Vec<Email>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Plan {
    pub(super) source: SourceFile,
    pub(super) dlc: String,
    pub(super) content: Content,
}

fn identifier(value: &str, dotted: bool) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        && (dotted || !value.contains('.'))
}

fn text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains('\0')
}

fn parse(bytes: &[u8], squad: bool, target: Target) -> Result<Content> {
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Merge manifest exceeds its size limit"
    );
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if squad {
        let manifest: SquadManifest =
            serde_json::from_slice(bytes).context("Invalid squadmate merge manifest")?;
        ensure!(
            manifest.game.target()? == target && matches!(target, Target::Le2 | Target::Le3),
            "Squadmate merge targets the wrong game"
        );
        ensure!(manifest.outfits.len() <= 1024, "Too many squadmate outfits");
        let names: &[&str] = if target == Target::Le2 {
            &[
                "Convict",
                "Garrus",
                "Geth",
                "Grunt",
                "Leading",
                "Mystic",
                "Professor",
                "Tali",
                "Thief",
                "Veteran",
                "Vixen",
                "Assassin",
            ]
        } else {
            &[
                "Liara", "Kaidan", "Ashley", "Garrus", "EDI", "Prothean", "Marine", "Tali",
            ]
        };
        for outfit in &manifest.outfits {
            ensure!(
                names.contains(&outfit.henchname.as_str()),
                "Unknown squadmate in outfit manifest"
            );
            ensure!(
                identifier(&outfit.henchpackage, false) && outfit.henchpackage.starts_with("BioH_"),
                "Invalid squadmate package name"
            );
            ensure!(
                identifier(&outfit.availableimage, true)
                    && (outfit.highlightimage.is_empty()
                        || identifier(&outfit.highlightimage, true))
                    && outfit
                        .silhouetteimage
                        .as_ref()
                        .is_none_or(|name| identifier(name, true)),
                "Invalid squadmate image reference"
            );
            ensure!(outfit.plotflag >= -1, "Invalid outfit availability flag");
        }
        Ok(Content::Outfits(manifest.outfits))
    } else {
        let manifest: EmailManifest =
            serde_json::from_slice(bytes).context("Invalid email merge manifest")?;
        ensure!(
            target == Target::Le2 && manifest.game.target()? == target,
            "Email merging requires LE2"
        );
        ensure!(
            manifest.emails.len() <= 4096 && text(&manifest.mod_name, 1024),
            "Invalid email manifest metadata"
        );
        ensure!(
            manifest.in_memory_bool.is_none_or(|id| id >= 0),
            "Invalid email availability flag"
        );
        let mut statuses = BTreeSet::new();
        for email in &manifest.emails {
            ensure!(
                text(&email.email_name, 1024) && text(&email.trigger_conditional, 64 * 1024),
                "Invalid email name or trigger"
            );
            ensure!(
                email.status_plot_int >= 0
                    && statuses.insert(email.status_plot_int)
                    && email.read_transition.is_none_or(|id| id >= 0),
                "Invalid or duplicate email status integer"
            );
        }
        Ok(Content::Emails {
            mod_name: manifest.mod_name,
            in_memory_bool: manifest.in_memory_bool,
            emails: manifest.emails,
        })
    }
}

pub(super) fn inspect(
    root: &Path,
    sources: &[SourceFile],
    files: &[FileMapping],
    target: Target,
    version: &str,
) -> Result<Vec<Plan>> {
    let mut plans = Vec::new();
    for file in files {
        let lower = file.destination.to_ascii_lowercase();
        let squad = lower.ends_with(".sqm");
        if !squad && !lower.ends_with(".emm") {
            continue;
        }
        let parts: Vec<_> = file.destination.split('/').collect();
        let canonical = if squad {
            "SquadmateMergeInfo.sqm"
        } else {
            "EmailMergeInfo.emm"
        };
        ensure!(
            parts.len() == 4
                && parts[0] == "DLC"
                && parts[2].eq_ignore_ascii_case("CookedPCConsole"),
            "Merge manifest must be directly inside a custom DLC CookedPCConsole directory"
        );
        super::manifest::validate_dlc(parts[1])?;
        ensure!(
            !target.is_official_dlc(parts[1])
                && !parts[1].eq_ignore_ascii_case(super::merge_dlc::NAME),
            "Merge manifest requires an author-owned custom DLC"
        );
        ensure!(
            parts[3] == canonical || version == "9.2",
            "Additional merge manifest filenames require moddesc 9.2"
        );
        let source = sources
            .iter()
            .find(|source| source.relative == file.source)
            .context("Merge manifest source is missing")?;
        let content = parse(&read_input(root, source, 1024 * 1024)?, squad, target)?;
        plans.push(Plan {
            source: source.clone(),
            dlc: parts[1].to_owned(),
            content,
        });
        ensure!(plans.len() <= 1024, "Too many merge manifests");
    }
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::*;

    const SQUAD: &[u8] = br#"{"game":"LE3","outfits":[{"henchname":"Liara","henchpackage":"BioH_Liara_Test","availableimage":"Images.Available"}]}"#;
    const EMAIL: &[u8] = br#"{"game":5,"modName":"Example","emails":[{"emailName":"Welcome","statusPlotInt":200,"triggerConditional":"(plot.ints[200] == i0)","titleStrRef":10,"descStrRef":11}]}"#;

    // @variants: both
    #[test]
    fn validates_manifest_layout_version_and_source_identity() -> Result<()> {
        let root = tempdir()?;
        std::fs::write(root.path().join("input.sqm"), SQUAD)?;
        let sources = [SourceFile {
            relative: "input.sqm".into(),
            size: SQUAD.len() as u64,
            sha256: format!("{:x}", Sha256::digest(SQUAD)),
        }];
        let mut files = [FileMapping {
            source: "input.sqm".into(),
            destination: "DLC/DLC_MOD_Test/CookedPCConsole/Additional.sqm".into(),
        }];
        assert!(inspect(root.path(), &sources, &files, Target::Le3, "9.1").is_err());
        assert_eq!(
            inspect(root.path(), &sources, &files, Target::Le3, "9.2")?.len(),
            1
        );
        for destination in [
            "../system/Additional.sqm",
            "~docs~/Additional.sqm",
            "Mods/Additional.sqm",
            "DLC/DLC_MOD_M3_MERGE/CookedPCConsole/Additional.sqm",
        ] {
            files[0].destination = destination.into();
            assert!(inspect(root.path(), &sources, &files, Target::Le3, "9.2").is_err());
        }
        files[0].destination = "DLC/DLC_MOD_Test/CookedPCConsole/SquadmateMergeInfo.sqm".into();
        assert_eq!(
            inspect(root.path(), &sources, &files, Target::Le3, "9.1")?.len(),
            1
        );
        std::fs::write(root.path().join("input.sqm"), b"changed")?;
        assert!(inspect(root.path(), &sources, &files, Target::Le3, "9.1").is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn reads_merge_defaults_and_rejects_other_games() -> Result<()> {
        let Content::Outfits(outfits) = parse(SQUAD, true, Target::Le3)? else {
            panic!("wrong merge kind")
        };
        assert_eq!(outfits[0].plotflag, -1);
        assert_eq!(outfits[0].silhouetteimage, None);
        assert!(parse(SQUAD, true, Target::Le2).is_err());
        assert!(parse(SQUAD, true, Target::Le1).is_err());
        assert!(matches!(
            parse(EMAIL, false, Target::Le2)?,
            Content::Emails { .. }
        ));
        assert!(parse(EMAIL, false, Target::Le3).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_unsafe_assets_duplicate_fields_and_unknown_semantics() -> Result<()> {
        let source = std::str::from_utf8(SQUAD)?;
        for changed in [
            source.replace("BioH_Liara_Test", "../BioH_Liara_Test"),
            source.replace("Images.Available", "../Images.Available"),
            source.replace("\"game\":\"LE3\"", "\"game\":\"LE3\",\"game\":\"LE3\""),
            source.replace("\"outfits\":", "\"futureOperation\":true,\"outfits\":"),
        ] {
            assert!(parse(changed.as_bytes(), true, Target::Le3).is_err());
        }
        let source = std::str::from_utf8(EMAIL)?.replace("\"game\":5", "\"game\":2");
        assert!(parse(source.as_bytes(), false, Target::Le2).is_err());
        Ok(())
    }
}
