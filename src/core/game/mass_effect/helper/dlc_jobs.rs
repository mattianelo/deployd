use std::collections::BTreeSet;

use anyhow::{Result, ensure};

use super::super::{Target, merge_dlc};
use super::files;
use super::protocol::{EmailMerge, FileIdentity, MovieEdit, OutfitMerge};

fn identifier(name: &str, dotted: bool) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && (dotted || !name.contains('.'))
        && name.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

pub(super) fn dlc(
    game: Target,
    inputs: &[FileIdentity],
    outfits: &[OutfitMerge],
    emails: &[EmailMerge],
    outputs: &[String],
) -> Result<()> {
    ensure!(
        !inputs.is_empty()
            && inputs.len() <= 4096
            && (1..=4096).contains(&(outfits.len() + emails.len())),
        "Invalid merge DLC input count"
    );
    ensure!(
        outputs == merge_dlc::outputs(game, !outfits.is_empty(), !emails.is_empty())?,
        "Generated merge DLC output inventory differs from its plan"
    );
    let mut paths = BTreeSet::new();
    for input in inputs {
        files::relative(&input.path)?;
        ensure!(
            paths.insert(input.path.to_lowercase()),
            "Duplicate merge DLC input"
        );
        let parts: Vec<_> = input.path.split('/').collect();
        let base = parts.len() == 2 && parts[0].eq_ignore_ascii_case("CookedPCConsole");
        let dlc = parts.len() == 4
            && parts[0].eq_ignore_ascii_case("DLC")
            && identifier(parts[1], false)
            && parts[1].to_ascii_uppercase().starts_with("DLC_")
            && !parts[1].eq_ignore_ascii_case(merge_dlc::NAME)
            && parts[2].eq_ignore_ascii_case("CookedPCConsole");
        ensure!(
            (base || dlc)
                && (input.path.to_ascii_lowercase().ends_with(".pcc")
                    || (dlc && parts[3].eq_ignore_ascii_case("Mount.dlc") && input.size <= 65536)),
            "Invalid merge DLC input path"
        );
    }
    let mut conditionals = BTreeSet::new();
    let mut slots = BTreeSet::new();
    let mut packages = BTreeSet::new();
    for outfit in outfits {
        ensure!(
            identifier(&outfit.dlc, false)
                && outfit.dlc.to_ascii_uppercase().starts_with("DLC_MOD_")
                && !outfit.dlc.eq_ignore_ascii_case(merge_dlc::NAME)
                && identifier(&outfit.hench_name, false)
                && identifier(&outfit.hench_package, false)
                && outfit.hench_package.starts_with("BioH_")
                && identifier(&outfit.available_image, true)
                && ((game == Target::Le3 && outfit.highlight_image.is_empty())
                    || identifier(&outfit.highlight_image, true))
                && outfit
                    .silhouette_image
                    .as_ref()
                    .is_none_or(|name| identifier(name, true))
                && outfit.plot_flag >= -1,
            "Invalid squadmate merge contribution"
        );
        ensure!(
            (10000..=14095).contains(&outfit.conditional)
                && conditionals.insert(outfit.conditional)
                && if game == Target::Le2 {
                    (2..=31).contains(&outfit.appearance)
                } else {
                    (255..=1278).contains(&outfit.appearance)
                },
            "Invalid squadmate appearance or conditional ID"
        );
        ensure!(
            slots.insert((outfit.hench_name.to_lowercase(), outfit.appearance))
                && packages.insert(outfit.hench_package.to_lowercase()),
            "Conflicting squadmate outfit contributions"
        );
    }
    let mut statuses = BTreeSet::new();
    let mut transitions = BTreeSet::new();
    for email in emails {
        ensure!(
            identifier(&email.dlc, false)
                && email.dlc.to_ascii_uppercase().starts_with("DLC_MOD_")
                && email.name.len() <= 1024
                && !email.name.contains('\0')
                && email.trigger.len() <= 65536
                && !email.trigger.contains('\0')
                && email.status >= 0
                && email.in_memory_bool.is_none_or(|id| id >= 0)
                && email.read_transition.is_none_or(|id| id >= 0)
                && (10000..=14095).contains(&email.conditional)
                && conditionals.insert(email.conditional)
                && (90000..=94095).contains(&email.transition)
                && transitions.insert(email.transition),
            "Invalid email merge contribution"
        );
        ensure!(
            statuses.insert(email.status),
            "Email mods use the same status integer; resolve their conflict before deploying"
        );
    }
    Ok(())
}

pub(super) fn ui(assets: &[FileIdentity], movies: &[MovieEdit]) -> Result<()> {
    ensure!(
        !assets.is_empty() && assets.len() <= 1024 && !movies.is_empty() && movies.len() <= 4,
        "Invalid squad UI input count"
    );
    let mut paths = BTreeSet::new();
    for input in assets {
        files::relative(&input.path)?;
        let parts: Vec<_> = input.path.split('/').collect();
        ensure!(
            parts.len() == 4
                && parts[0] == "DLC"
                && identifier(parts[1], false)
                && parts[1].starts_with("DLC_MOD_")
                && !parts[1].eq_ignore_ascii_case(merge_dlc::NAME)
                && parts[2] == "CookedPCConsole"
                && parts[3] == format!("SFXHenchImages_{}.pcc", parts[1])
                && paths.insert(input.path.clone()),
            "Invalid or duplicate squad UI asset ownership"
        );
    }
    let mut targets = BTreeSet::new();
    let mut used = BTreeSet::new();
    for movie in movies {
        let name = movie
            .target
            .path
            .strip_prefix(merge_dlc::COOKED)
            .unwrap_or_default();
        ensure!(
            merge_dlc::UI_PACKAGES.contains(&name)
                && targets.insert(name)
                && movie.movie.path == format!(".merge-ui/{name}.gfx")
                && movie.movie.size <= 16 * 1024 * 1024
                && (2..=720).contains(&movie.images.len()),
            "Invalid squad UI target"
        );
        let mut destinations = BTreeSet::new();
        for image in &movie.images {
            let suffix = image
                .destination
                .strip_prefix("TeamSelect_I")
                .unwrap_or_default();
            ensure!(
                paths.contains(&image.package)
                    && identifier(&image.export, true)
                    && (1..=4).contains(&suffix.len())
                    && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && destinations.insert(image.destination.to_lowercase()),
                "Invalid squad UI texture mapping"
            );
            used.insert(image.package.clone());
        }
    }
    ensure!(
        used == paths,
        "Squad UI request contains unused image packages"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str) -> FileIdentity {
        FileIdentity {
            path: path.into(),
            size: 100,
            sha256: "a".repeat(64),
        }
    }

    fn outfit() -> OutfitMerge {
        OutfitMerge {
            dlc: "DLC_MOD_Test".into(),
            hench_name: "Liara".into(),
            hench_package: "BioH_Liara_Test".into(),
            available_image: "Images.Available".into(),
            highlight_image: "Images.Highlight".into(),
            silhouette_image: None,
            description_text: 0,
            custom_token: 0,
            plot_flag: -1,
            appearance: 255,
            conditional: 10000,
        }
    }

    // @variants: both
    #[test]
    fn rejects_foreign_outputs_and_conflicting_outfit_slots() -> Result<()> {
        let inputs = vec![input("CookedPCConsole/BioP_Global.pcc")];
        let outputs = merge_dlc::outputs(Target::Le3, true, false)?;
        dlc(Target::Le3, &inputs, &[outfit()], &[], &outputs)?;
        let mut foreign = outputs.clone();
        foreign.push("../system/Engine.dll".into());
        assert!(dlc(Target::Le3, &inputs, &[outfit()], &[], &foreign).is_err());
        assert!(dlc(Target::Le1, &inputs, &[outfit()], &[], &outputs).is_err());
        let mut duplicate = outfit();
        duplicate.conditional += 1;
        assert!(dlc(Target::Le3, &inputs, &[outfit(), duplicate], &[], &outputs).is_err());
        for path in [
            "../Engine.pcc",
            "~docs~/Engine.pcc",
            "DLC/DLC_MOD_M3_MERGE/CookedPCConsole/BioP_Global.pcc",
        ] {
            assert!(dlc(Target::Le3, &[input(path)], &[outfit()], &[], &outputs).is_err());
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_shared_email_status_and_undeclared_ui_images() -> Result<()> {
        let email = EmailMerge {
            dlc: "DLC_MOD_Test".into(),
            name: "Test".into(),
            status: 100,
            trigger: String::new(),
            title: 1,
            description: 2,
            read_transition: None,
            in_memory_bool: None,
            conditional: 10000,
            transition: 90000,
        };
        let inputs = vec![input("CookedPCConsole/BioD_Nor_103Messages.pcc")];
        let outputs = merge_dlc::outputs(Target::Le2, false, true)?;
        dlc(
            Target::Le2,
            &inputs,
            &[],
            std::slice::from_ref(&email),
            &outputs,
        )?;
        let mut other = email.clone();
        other.conditional += 1;
        other.transition += 1;
        assert!(dlc(Target::Le2, &inputs, &[], &[email, other], &outputs).is_err());
        let asset = input("DLC/DLC_MOD_Test/CookedPCConsole/SFXHenchImages_DLC_MOD_Test.pcc");
        let mut movie = MovieEdit {
            target: input(&format!("{}BioH_SelectGUI.pcc", merge_dlc::COOKED)),
            movie: input(".merge-ui/BioH_SelectGUI.pcc.gfx"),
            images: vec![
                super::super::protocol::TextureCopy {
                    package: asset.path.clone(),
                    export: "Images.Available".into(),
                    destination: "TeamSelect_I200".into(),
                },
                super::super::protocol::TextureCopy {
                    package: asset.path.clone(),
                    export: "Images.Highlight".into(),
                    destination: "TeamSelect_I201".into(),
                },
            ],
        };
        ui(std::slice::from_ref(&asset), std::slice::from_ref(&movie))?;
        movie.images[1].package = "Other.pcc".into();
        assert!(ui(&[asset], &[movie]).is_err());
        Ok(())
    }
}
