use anyhow::{Result, ensure};

use super::Target;
use super::baseline::Baseline;
use super::package::FileMapping;

pub(super) const NAME: &str = "DLC_MOD_M3_MERGE";
pub(super) const MOUNT: i32 = 45_824;
pub(super) const COOKED: &str = "DLC/DLC_MOD_M3_MERGE/CookedPCConsole/";
pub(super) const UI_PACKAGES: &[&str] = &[
    "BioH_SelectGUI.pcc",
    "BioP_Exp1Lvl2.pcc",
    "BioP_Exp1Lvl3.pcc",
    "BioP_Exp1Lvl4.pcc",
];

pub(super) fn outputs(game: Target, outfits: bool, emails: bool) -> Result<Vec<String>> {
    ensure!(
        matches!(game, Target::Le2 | Target::Le3)
            && (outfits || emails)
            && (game == Target::Le2 || !emails),
        "Unsupported merge DLC game or contents"
    );
    let mut names = vec!["Mount.dlc".to_string()];
    let prefix = if game == Target::Le2 {
        "DLC_48955"
    } else {
        NAME
    };
    names.extend(
        ["INT", "DEU", "FRA", "ITA", "ESN", "RUS", "POL", "JPN"]
            .iter()
            .map(|language| format!("{prefix}_{language}.tlk")),
    );
    if game == Target::Le3 {
        names.push(format!("Default_{NAME}.bin"));
    } else {
        names.extend([
            "BIOEngine.ini".into(),
            "BIOGame.ini".into(),
            format!("Startup_{NAME}.pcc"),
        ]);
    }
    if outfits {
        names.push("BioP_Global.pcc".into());
        if game == Target::Le3 {
            names.push(format!("Conditionals{NAME}.cnd"));
        } else {
            names.extend(
                [
                    "BIOUI.ini",
                    "BioP_EndGm_StuntHench.pcc",
                    "BioD_ZyaVTL_110Jungle.pcc",
                ]
                .into_iter()
                .chain(UI_PACKAGES.iter().copied())
                .map(str::to_owned),
            );
        }
    }
    if emails {
        names.push("BioD_Nor_103Messages.pcc".into());
    }
    let mut paths: Vec<_> = names
        .into_iter()
        .map(|name| format!("{COOKED}{name}"))
        .collect();
    if game == Target::Le2 && outfits {
        paths.extend(
            UI_PACKAGES
                .iter()
                .map(|name| format!(".merge-ui/{name}.gfx")),
        );
    }
    paths.sort();
    Ok(paths)
}

pub(super) fn contains(path: &str) -> bool {
    let mut parts = path.split('/');
    let mut first = parts.next().unwrap_or_default();
    if first.eq_ignore_ascii_case("BioGame") {
        first = parts.next().unwrap_or_default();
    }
    first.eq_ignore_ascii_case("DLC")
        && parts
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(NAME))
}

pub(super) fn baseline(baseline: &Baseline) -> Result<()> {
    ensure!(
        !baseline.files.iter().any(|file| contains(&file.relative)),
        "The restoration point contains an externally managed merge DLC; restore clean content before adding this game"
    );
    Ok(())
}

pub(super) fn archive(files: &[FileMapping]) -> Result<()> {
    ensure!(
        !files.iter().any(|file| contains(&file.destination)),
        "DLC_MOD_M3_MERGE is reserved for generated merges; archives cannot install files into it"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn reserved_merge_paths_are_case_insensitive_and_do_not_capture_other_roots() {
        for path in [
            "DLC/DLC_MOD_M3_MERGE",
            "BioGame/DLC/dlc_mod_m3_merge/CookedPCConsole/test.pcc",
        ] {
            assert!(contains(path));
        }
        for path in [
            "DLC/DLC_MOD_M3_MERGE_Extra/file",
            "../system/DLC_MOD_M3_MERGE/file",
            "~docs~/DLC_MOD_M3_MERGE/file",
            "Mods/DLC_MOD_M3_MERGE/file",
        ] {
            assert!(!contains(path));
        }
    }
}
