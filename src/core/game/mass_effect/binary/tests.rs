use std::fs;

use super::*;

pub(in crate::core::game::mass_effect) fn plugin() -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
    bytes[70..72].copy_from_slice(&1u16.to_le_bytes());
    bytes[84..86].copy_from_slice(&112u16.to_le_bytes());
    bytes[86..88].copy_from_slice(&0x2002u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes[216..220].copy_from_slice(&16u32.to_le_bytes());
    bytes[220..224].copy_from_slice(&256u32.to_le_bytes());
    bytes
}

// @variants: both
#[test]
fn rejects_wrong_architecture_executables_and_truncated_sections() -> Result<()> {
    let bytes = plugin();
    pe(&bytes)?;
    for (offset, replacement) in [(68, 0x14cu16), (86, 2), (88, 0x10b)] {
        let mut invalid = bytes.clone();
        invalid[offset..offset + 2].copy_from_slice(&replacement.to_le_bytes());
        assert!(pe(&invalid).is_err());
    }
    for length in [0, 63, 68, 87, 199, 239, 271] {
        assert!(pe(&bytes[..length]).is_err());
    }
    let mut invalid = bytes;
    invalid[220..224].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(pe(&invalid).is_err());
    Ok(())
}

// @variants: both
#[test]
fn root_plugins_preserve_content_anchors_and_protect_managed_runtimes() -> Result<()> {
    assert_eq!(
        game_path("CookedPCConsole/Engine.pcc")?,
        "BioGame/CookedPCConsole/Engine.pcc"
    );
    for path in [
        "Binaries/Win64/ASI/Test.asi",
        "Binaries/Win64/Test.dll",
        "Binaries/Win64/ASI/Test.ini",
    ] {
        assert_eq!(game_path(&encode(path)?)?, path);
    }
    for path in [
        "../system/Test.dll",
        "../launcher/Test.dll",
        "~docs~/Test.dll",
        "Mods/Test.dll",
        "Binaries/Win64/../Test.dll",
        "Binaries/Win64/Test.exe",
        "Binaries/Win64/Test.sh",
        "Binaries/Win64/Test.asi",
        "Binaries/Win64/ASI/Sub/Test.asi",
        "Binaries/Win64/bink2w64.dll",
        "Binaries/Win64/BINK2W64_ORIGINAL.DLL",
        "Binaries/Win64/oo2core_9_win64.dll",
        "Binaries/Win64/ASI/AutoTOCLE-v2.asi",
        "Binaries/Win64/vcruntime140.dll",
    ] {
        assert!(encode(path).is_err(), "{path}");
    }
    assert!(
        mapping(&FileMapping {
            source: "Test.dll".into(),
            destination: "CookedPCConsole/Test.pcc".into()
        })
        .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn manual_plugins_require_a_target_and_approval_bound_to_the_source() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("Test.asi"), plugin())?;
    fs::write(temp.path().join("README.txt"), b"Instructions")?;
    assert!(PackagePlan::inspect(temp.path(), None).is_err());
    let first = PackagePlan::inspect(temp.path(), Some(Target::Le1))?;
    assert_eq!(first.files.len(), 1);
    assert!(first.needs_binary_approval());
    let approval = Approval::for_plan(&first);
    assert!(approval.matches(&first));
    let other = PackagePlan::inspect(temp.path(), Some(Target::Le2))?;
    assert!(!approval.matches(&other));
    fs::write(temp.path().join("README.txt"), b"Changed source")?;
    let changed = PackagePlan::inspect(temp.path(), Some(Target::Le1))?;
    assert!(!approval.matches(&changed));
    fs::write(temp.path().join("setup.exe"), plugin())?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le1)).is_err());
    Ok(())
}

// @variants: both
#[test]
fn validates_renamed_binary_payloads_before_installation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("plugin.dat"), plugin())?;
    fs::write(
        temp.path().join("moddesc.ini"),
        "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE2\nmodname=Plugin\nmodver=1.0\nmoddev=Author\nmoddesc=Plugin\n[BASEGAME]\nmoddir=.\nnewfiles=plugin.dat\nreplacefiles=Binaries/Win64/Test.dll\n",
    )?;
    let plan = PackagePlan::inspect(temp.path(), Some(Target::Le2))?;
    assert!(plan.needs_binary_approval());
    fs::write(temp.path().join("plugin.dat"), b"Not a plugin")?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    Ok(())
}

// @variants: both
#[test]
fn rejects_plugin_links_and_case_collisions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("Test.asi"), plugin())?;
    fs::write(temp.path().join("test.asi"), plugin())?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le1)).is_err());
    fs::remove_file(temp.path().join("test.asi"))?;
    std::os::unix::fs::symlink(temp.path().join("Test.asi"), temp.path().join("Other.asi"))?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le1)).is_err());
    Ok(())
}

// @variants: both
#[test]
fn inspects_inactive_binary_choices_and_multilist_companions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let header = "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE3\nmodname=Optional plugin\nmodver=1.0\nmoddev=Author\nmoddesc=Options\n[BASEGAME]\nmoddir=.\nnewfiles=content.pcc\nreplacefiles=BioGame/CookedPCConsole/Test.pcc\n";
    fs::write(temp.path().join("content.pcc"), b"content")?;
    fs::write(temp.path().join("plugin.dat"), plugin())?;
    fs::write(
        temp.path().join("moddesc.ini"),
        format!(
            "{header}altfiles=((FriendlyName=Plugin,Condition=COND_MANUAL,ModOperation=OP_INSTALL,ModFile=Binaries/Win64/Optional.dll,AltFile=plugin.dat))\n"
        ),
    )?;
    let mut plan = PackagePlan::inspect(temp.path(), Some(Target::Le3))?;
    assert!(plan.needs_binary_approval());
    assert_eq!(plan.files.len(), 1);
    plan.resolve(temp.path(), &plan.option_keys(), &Default::default())?;
    assert_eq!(plan.files.len(), 2);
    fs::write(temp.path().join("plugin.dat"), b"invalid unselected DLL")?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le3)).is_err());
    fs::remove_file(temp.path().join("plugin.dat"))?;
    fs::create_dir_all(temp.path().join("Options"))?;
    fs::write(temp.path().join("Options/Example.ini"), b"[Settings]")?;
    fs::write(
        temp.path().join("moddesc.ini"),
        format!(
            "{header}multilist1=Example.ini\naltfiles=((FriendlyName=Config,Condition=COND_MANUAL,ModOperation=OP_APPLY_MULTILISTFILES,MultiListId=1,MultiListRootPath=Options,MultiListTargetPath=Binaries/Win64/ASI))\n"
        ),
    )?;
    let mut plan = PackagePlan::inspect(temp.path(), Some(Target::Le3))?;
    assert!(plan.needs_binary_approval());
    plan.resolve(temp.path(), &plan.option_keys(), &Default::default())?;
    assert!(
        plan.files
            .iter()
            .any(|file| file.destination == "~game~/Binaries/Win64/ASI/Example.ini")
    );
    Ok(())
}
