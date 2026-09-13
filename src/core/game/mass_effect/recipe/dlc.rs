use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use super::super::helper::protocol::{EmailMerge, OutfitMerge};
use super::super::helper::{FileIdentity, Job};
use super::super::merge_manifest::{Content, Plan};
use super::super::{Target, merge_dlc};
use super::{Baseline, Control, PlannedFile};

pub(super) struct Dlc {
    game: Target,
    inputs: BTreeSet<String>,
    pub(super) outfits: Vec<OutfitMerge>,
    emails: Vec<EmailMerge>,
}

impl Dlc {
    pub(super) fn inspect(
        contributions: Vec<(Target, i32, Plan)>,
        files: &[PlannedFile],
        baseline: &Baseline,
        active_dlc: &BTreeSet<String>,
        originals: &mut BTreeMap<String, FileIdentity>,
        control: &Control,
    ) -> Result<Option<Self>> {
        let Some(mut result) = Self::contributions(contributions, control)? else {
            return Ok(None);
        };
        let game = result.game;
        let mut names = BTreeSet::new();
        if game == Target::Le2 {
            names.extend(
                super::super::helper::jobs::compiler_bases(game)
                    .iter()
                    .map(|name| name.to_lowercase()),
            );
        }
        if !result.emails.is_empty() {
            names.insert("biod_nor_103messages.pcc".into());
        }
        if !result.outfits.is_empty() {
            names.insert("biop_global.pcc".into());
            if game == Target::Le2 {
                names.extend(
                    merge_dlc::UI_PACKAGES
                        .iter()
                        .map(|name| name.to_lowercase()),
                );
                names.extend([
                    "biop_endgm_stunthench.pcc".into(),
                    "biod_zyavtl_110jungle.pcc".into(),
                ]);
            }
        }
        for outfit in &result.outfits {
            names.insert(format!("{}.pcc", outfit.hench_package).to_lowercase());
            if game == Target::Le3 {
                names.insert(format!("{}_Explore.pcc", outfit.hench_package).to_lowercase());
            } else {
                names.insert(
                    format!(
                        "BioH_END_{}.pcc",
                        outfit
                            .hench_package
                            .strip_prefix("BioH_")
                            .context("Invalid squadmate package name")?
                    )
                    .to_lowercase(),
                );
            }
            names.insert(format!("SFXHenchImages_{}.pcc", outfit.dlc).to_lowercase());
        }
        let available = |path: &str| {
            let parts: Vec<_> = path.split('/').collect();
            parts.len() == 3
                && parts[0] == "BioGame"
                && parts[1].eq_ignore_ascii_case("CookedPCConsole")
                || parts.len() == 5
                    && parts[0] == "BioGame"
                    && parts[1].eq_ignore_ascii_case("DLC")
                    && active_dlc.contains(&parts[2].to_lowercase())
                    && parts[3].eq_ignore_ascii_case("CookedPCConsole")
        };
        let mut candidates = BTreeMap::new();
        for file in &baseline.files {
            if available(&file.relative) {
                candidates.insert(
                    file.relative.to_lowercase(),
                    FileIdentity {
                        path: file.relative[8..].into(),
                        size: file.size,
                        sha256: file.sha256.clone(),
                    },
                );
            }
        }
        for file in files {
            if available(&file.destination.relative) {
                candidates.insert(
                    file.destination.relative.to_lowercase(),
                    super::merges::identity(&file.destination)?,
                );
            }
        }
        let mut mounts = BTreeSet::new();
        let mut found = BTreeSet::new();
        for (path, file) in &candidates {
            let name = path.rsplit('/').next().unwrap_or_default();
            if names.contains(name) {
                found.insert(name.to_owned());
                result.inputs.insert(file.path.clone());
                let parts: Vec<_> = file.path.split('/').collect();
                if parts.len() == 4 {
                    mounts.insert(format!(
                        "biogame/dlc/{}/cookedpcconsole/mount.dlc",
                        parts[1].to_lowercase()
                    ));
                }
            }
        }
        ensure!(
            found == names,
            "Merge DLC requires missing game or mod packages: {}",
            names
                .difference(&found)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        for mount in mounts {
            result.inputs.insert(
                candidates
                    .get(&mount)
                    .with_context(|| format!("Merge DLC requires a mount file for '{mount}'"))?
                    .path
                    .clone(),
            );
        }
        for path in &result.inputs {
            if let Some(file) = baseline
                .files
                .iter()
                .find(|file| file.relative == format!("BioGame/{path}"))
            {
                originals.insert(
                    path.clone(),
                    FileIdentity {
                        path: path.clone(),
                        size: file.size,
                        sha256: file.sha256.clone(),
                    },
                );
            }
        }
        Ok(Some(result))
    }

    fn contributions(
        mut contributions: Vec<(Target, i32, Plan)>,
        control: &Control,
    ) -> Result<Option<Self>> {
        let Some((game, _, _)) = contributions.first() else {
            return Ok(None);
        };
        let game = *game;
        contributions.sort_by_key(|(_, mount, plan)| (*mount, plan.source.relative.to_lowercase()));
        let mut result = Self {
            game,
            inputs: BTreeSet::new(),
            outfits: Vec::new(),
            emails: Vec::new(),
        };
        let mut appearances = BTreeMap::new();
        let mut conditional = 10000;
        for (target, mount, plan) in &contributions {
            control.check()?;
            ensure!(
                *target == game && *mount < merge_dlc::MOUNT,
                "Squadmate and email contributions must target the same game and mount below the generated merge DLC ({})",
                merge_dlc::MOUNT
            );
            if let Content::Outfits(outfits) = &plan.content {
                for outfit in outfits {
                    let appearance = if game == Target::Le3 {
                        255 + result.outfits.len() as i32
                    } else {
                        let next =
                            appearances
                                .entry(outfit.henchname.clone())
                                .or_insert_with(|| {
                                    if ["Vixen", "Garrus", "Grunt", "Tali", "Convict", "Assassin"]
                                        .contains(&outfit.henchname.as_str())
                                    {
                                        3
                                    } else {
                                        2
                                    }
                                });
                        let value = *next;
                        *next += 1;
                        value
                    };
                    result.outfits.push(OutfitMerge {
                        dlc: plan.dlc.clone(),
                        hench_name: outfit.henchname.clone(),
                        hench_package: outfit.henchpackage.clone(),
                        available_image: outfit.availableimage.clone(),
                        highlight_image: outfit.highlightimage.clone(),
                        silhouette_image: outfit.silhouetteimage.clone(),
                        description_text: outfit.descriptiontext0,
                        custom_token: outfit.customtoken0,
                        plot_flag: outfit.plotflag,
                        appearance,
                        conditional,
                    });
                    conditional += 1;
                }
            }
        }
        contributions.sort_by_key(|(_, mount, plan)| {
            (
                std::cmp::Reverse(*mount),
                plan.source.relative.to_lowercase(),
            )
        });
        for (_, _, plan) in &contributions {
            if let Content::Emails {
                in_memory_bool,
                emails,
                ..
            } = &plan.content
            {
                for email in emails {
                    result.emails.push(EmailMerge {
                        dlc: plan.dlc.clone(),
                        name: email.email_name.clone(),
                        status: email.status_plot_int,
                        trigger: email.trigger_conditional.clone(),
                        title: email.title_str_ref,
                        description: email.desc_str_ref,
                        read_transition: email.read_transition,
                        in_memory_bool: *in_memory_bool,
                        conditional,
                        transition: 90000 + result.emails.len() as i32,
                    });
                    conditional += 1;
                }
            }
        }
        if result.outfits.is_empty() && result.emails.is_empty() {
            return Ok(None);
        }
        Ok(Some(result))
    }

    pub(super) fn outputs(&self) -> Result<Vec<String>> {
        merge_dlc::outputs(self.game, !self.outfits.is_empty(), !self.emails.is_empty())
    }

    pub(super) fn job(&self, current: &BTreeMap<String, FileIdentity>) -> Result<Job> {
        Ok(Job::Dlc {
            game: self.game,
            inputs: self
                .inputs
                .iter()
                .map(|path| {
                    current
                        .get(path)
                        .cloned()
                        .with_context(|| format!("Missing merge DLC input '{path}'"))
                })
                .collect::<Result<_>>()?,
            outfits: self.outfits.clone(),
            emails: self.emails.clone(),
            outputs: self.outputs()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::package::SourceFile;
    use super::*;

    fn control() -> Control {
        Control::recovery()
    }

    fn plan(dlc: &str, source: &str, content: Content) -> Plan {
        Plan {
            source: SourceFile {
                relative: source.into(),
                size: 1,
                sha256: "a".repeat(64),
            },
            dlc: dlc.into(),
            content,
        }
    }

    fn outfits(dlc: &str) -> Plan {
        let outfit = serde_json::from_value(
            serde_json::json!({"henchname":"Vixen","henchpackage":format!("BioH_Vixen_{dlc}"),
            "availableimage":"Images.Available","highlightimage":"Images.Highlight"}),
        )
        .unwrap();
        plan(
            dlc,
            "SquadmateMergeInfo.sqm",
            Content::Outfits(vec![outfit]),
        )
    }

    fn emails(dlc: &str, source: &str, status: i32) -> Plan {
        let email = serde_json::from_value(
            serde_json::json!({"statusPlotInt":status,"titleStrRef":1,"descStrRef":2}),
        )
        .unwrap();
        plan(
            dlc,
            source,
            Content::Emails {
                mod_name: dlc.into(),
                in_memory_bool: None,
                emails: vec![email],
            },
        )
    }

    // @variants: both
    #[test]
    fn orders_outfits_and_email_manifests_by_their_mount_semantics() -> Result<()> {
        let result = Dlc::contributions(
            vec![
                (Target::Le2, 20, outfits("DLC_MOD_High")),
                (
                    Target::Le2,
                    10,
                    emails("DLC_MOD_Low", "EmailMergeInfo.emm", 1),
                ),
                (Target::Le2, 20, emails("DLC_MOD_High", "B.emm", 3)),
                (Target::Le2, 10, outfits("DLC_MOD_Low")),
                (Target::Le2, 20, emails("DLC_MOD_High", "A.emm", 2)),
            ],
            &control(),
        )?
        .context("Missing contributions")?;
        assert_eq!(
            result
                .outfits
                .iter()
                .map(|outfit| (&*outfit.dlc, outfit.appearance, outfit.conditional))
                .collect::<Vec<_>>(),
            vec![("DLC_MOD_Low", 3, 10000), ("DLC_MOD_High", 4, 10001)]
        );
        assert_eq!(
            result
                .emails
                .iter()
                .map(|email| (email.status, email.conditional, email.transition))
                .collect::<Vec<_>>(),
            vec![(2, 10002, 90000), (3, 10003, 90001), (1, 10004, 90002)]
        );
        Ok(())
    }

    #[test]
    fn rejects_contributions_that_outmount_the_generated_dlc() {
        assert!(
            Dlc::contributions(
                vec![(Target::Le2, merge_dlc::MOUNT, outfits("DLC_MOD_Test"))],
                &control()
            )
            .is_err()
        );
        assert!(
            Dlc::contributions(
                vec![
                    (Target::Le2, 1, outfits("DLC_MOD_First")),
                    (Target::Le3, 2, outfits("DLC_MOD_Second"))
                ],
                &control()
            )
            .is_err()
        );
    }
}
