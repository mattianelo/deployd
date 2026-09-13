use super::*;

// @variants: both
#[test]
fn alternate_conditions_follow_presence_and_manual_selections() -> Result<()> {
    for (condition, expected) in [
        ("COND_DLC_PRESENT", true),
        ("COND_DLC_NOT_PRESENT", true),
        ("COND_ANY_DLC_PRESENT", true),
        ("COND_ANY_DLC_NOT_PRESENT", true),
        ("COND_ALL_DLC_PRESENT", false),
        ("COND_ALL_DLC_NOT_PRESENT", false),
    ] {
        let text = format!(
            "((FriendlyName=Compatibility,Condition={condition},ConditionalDLC=DLC_MOD_A;DLC_MOD_B,ModOperation=OP_NOTHING))"
        );
        let alternate = Alternate::parse(Some(&text), true, Target::Le1)?.remove(0);
        assert_eq!(
            alternate.active(&BTreeSet::new(), &BTreeSet::from(["dlc_mod_a".into()])),
            expected
        );
    }
    let alternate = Alternate::parse(Some("((FriendlyName=Manual,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_NOTHING))"), true, Target::Le1)?.remove(0);
    let absent = Alternate::parse(Some("((FriendlyName=Absent,Condition=COND_DLC_NOT_PRESENT,ConditionalDLC=DLC_MOD_A;DLC_MOD_B,ModOperation=OP_NOTHING))"), false, Target::Le1)?.remove(0);
    assert!(!absent.active(&BTreeSet::new(), &BTreeSet::from(["dlc_mod_a".into()])));
    assert!(alternate.default);
    assert!(!alternate.active(&BTreeSet::new(), &BTreeSet::new()));
    assert!(alternate.active(&BTreeSet::from([alternate.key.clone()]), &BTreeSet::new()));
    Ok(())
}

// @variants: both
#[test]
fn alternate_parser_preserves_quoted_text_and_rejects_unknown_semantics() -> Result<()> {
    let good = "((FriendlyName=\"A, B (optional)\",Description=\"Use A=B\",Condition=COND_MANUAL,ModOperation=OP_NOTHING))";
    let parsed = Alternate::parse(Some(good), true, Target::Le1)?;
    assert_eq!(parsed[0].name, "A, B (optional)");
    assert_eq!(parsed[0].description, "Use A=B");
    for bad in [
        good.replace("OP_NOTHING", "OP_FUTURE"),
        good.replace("COND_MANUAL", "COND_FUTURE"),
        good.replace("Condition=", "Hidden=invalid,Condition="),
        good.replace("Condition=", "condition=COND_MANUAL,Condition="),
        good.replace(
            "OP_NOTHING",
            "OP_ADD_CUSTOMDLC,ModAltDLC=../escape,ModDestDLC=DLC_MOD_X",
        ),
        good.replace(
            "OP_NOTHING",
            "OP_ADD_CUSTOMDLC,ModAltDLC=folder,ModDestDLC=DLC_UNC/../escape",
        ),
        good.replace("))", "),)"),
        good.replace("Use A=B\"", "Use A=B"),
    ] {
        assert!(
            Alternate::parse(Some(&bad), true, Target::Le1).is_err(),
            "{bad}"
        );
    }
    Ok(())
}

// @variants: both
#[test]
fn alternate_folder_overlays_regenerate_merges_and_validate_unused_sources() -> Result<()> {
    use std::fs;
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("Base/CookedPCConsole"))?;
    fs::create_dir_all(root.path().join("Overlay/CookedPCConsole"))?;
    fs::write(
        root.path().join("Base/AutoLoad.ini"),
        "[ME1DLCMOUNT]\nModMount=5",
    )?;
    fs::write(root.path().join("Base/CookedPCConsole/A.pcc"), b"base")?;
    fs::write(
        root.path().join("Overlay/CookedPCConsole/A.pcc"),
        b"overlay",
    )?;
    fs::write(
        root.path()
            .join("Overlay/CookedPCConsole/ConfigDelta-test.m3cd"),
        "[BioUI.ini Engine.UI]\n+Key=Value",
    )?;
    fs::write(
        root.path().join("moddesc.ini"),
        "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE1\nmodname=Options\nmodver=1.0\nmoddev=Deployd\nmoddesc=Fixture\n[CUSTOMDLC]\nsourcedirs=Base\ndestdirs=DLC_MOD_X\naltdlc=((FriendlyName=Overlay,Condition=COND_MANUAL,ModOperation=OP_ADD_FOLDERFILES_TO_CUSTOMDLC,ModAltDLC=Overlay,ModDestDLC=DLC_MOD_X))\n",
    )?;
    let original = PackagePlan::inspect(root.path(), None)?;
    assert!(original.m3cd.is_empty());
    let mut selected = original.clone();
    selected.resolve(root.path(), &original.option_keys(), &BTreeSet::new())?;
    assert_eq!(selected.m3cd.len(), 1);
    assert_eq!(selected.m3cd[0].edits[0].value, "Value");
    assert!(selected.files.iter().any(|file| file.destination
        == "DLC/DLC_MOD_X/CookedPCConsole/A.pcc"
        && file.source == "Overlay/CookedPCConsole/A.pcc"));
    let mut disabled = original.clone();
    disabled.resolve(root.path(), &BTreeSet::new(), &BTreeSet::new())?;
    assert!(disabled.m3cd.is_empty());
    fs::remove_dir_all(root.path().join("Overlay"))?;
    assert!(PackagePlan::inspect(root.path(), None).is_err());
    Ok(())
}

// @variants: both
#[test]
fn exclusive_groups_require_one_default_and_one_saved_choice() -> Result<()> {
    let text = "((FriendlyName=Standard,Condition=COND_MANUAL,OptionGroup=Appearance,CheckedByDefault=true,ModOperation=OP_NOTHING),(FriendlyName=Alternative,Condition=COND_MANUAL,OptionGroup=Appearance,ModOperation=OP_NOTHING))";
    let options = Alternate::parse(Some(text), true, Target::Le1)?;
    groups::validate(&options)?;
    for option in &options {
        groups::validate_choices(&options, &BTreeSet::from([option.key.clone()]))?;
    }
    assert!(groups::validate_choices(&options, &BTreeSet::new()).is_err());
    assert!(
        groups::validate_choices(
            &options,
            &options.iter().map(|option| option.key.clone()).collect()
        )
        .is_err()
    );
    let none = Alternate::parse(
        Some(&text.replace("CheckedByDefault=true,", "")),
        true,
        Target::Le1,
    )?;
    assert!(groups::validate(&none).is_err());
    let multiple = Alternate::parse(
        Some(&text.replace(
            "FriendlyName=Alternative,",
            "FriendlyName=Alternative,CheckedByDefault=true,",
        )),
        true,
        Target::Le1,
    )?;
    assert!(groups::validate(&multiple).is_err());
    assert!(
        Alternate::parse(
            Some(&text.replace("COND_MANUAL", "COND_DLC_PRESENT,ConditionalDLC=DLC_MOD_X")),
            true,
            Target::Le1
        )
        .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn multilists_validate_sources_exclusions_flattening_and_versions() -> Result<()> {
    use std::fs;
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("Base/CookedPCConsole"))?;
    fs::create_dir_all(root.path().join("Options/a"))?;
    fs::create_dir_all(root.path().join("Options/b"))?;
    fs::write(root.path().join("Base/CookedPCConsole/Old.pcc"), b"old")?;
    fs::write(root.path().join("Options/a/New.pcc"), b"new")?;
    fs::write(root.path().join("Options/b/New.pcc"), b"duplicate")?;
    let text = "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE1\nmodname=Multilist\nmodver=1.0\nmoddev=Deployd\nmoddesc=Fixture\n[CUSTOMDLC]\nsourcedirs=Base\ndestdirs=DLC_MOD_X\nmultilist1=a/New.pcc\nmultilist2=Old.pcc;Absent.pcc\naltdlc=((FriendlyName=Add,Condition=COND_MANUAL,CheckedByDefault=true,ModOperation=OP_ADD_MULTILISTFILES_TO_CUSTOMDLC,MultiListId=1,MultiListRootPath=Options,ModDestDLC=DLC_MOD_X/CookedPCConsole,FlattenMultiListOutput=true))\naltfiles=((FriendlyName=Exclude,Condition=COND_ALWAYS,ModOperation=OP_NOINSTALL_MULTILISTFILES,MultiListId=2,MultiListTargetPath=DLC_MOD_X/CookedPCConsole))\n";
    let manifest = root.path().join("moddesc.ini");
    fs::write(&manifest, text)?;
    let original = PackagePlan::inspect(root.path(), None)?;
    let mut plan = original.clone();
    plan.resolve(root.path(), &plan.default_options(), &BTreeSet::new())?;
    assert_eq!(
        plan.files,
        vec![FileMapping {
            source: "Options/a/New.pcc".into(),
            destination: "DLC/DLC_MOD_X/CookedPCConsole/New.pcc".into()
        }]
    );
    for invalid in [
        text.replace("multilist1=a/New.pcc", "multilist1=a/New.pcc;b/New.pcc"),
        text.replace("multilist1=a/New.pcc", "multilist1=../escape.pcc"),
        text.replace("multilist1=a/New.pcc", "multilist1=missing.pcc"),
        text.replace("MultiListId=1", "MultiListId=3"),
        text.replace("cmmver=9.1", "cmmver=7.0"),
        text.replace(
            "FlattenMultiListOutput=true",
            "FlattenMultiListOutput=perhaps",
        ),
    ] {
        fs::write(&manifest, invalid)?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
    }
    fs::write(
        &manifest,
        text.replace(
            "FlattenMultiListOutput=true",
            "FlattenMultiListOutput=false",
        ),
    )?;
    let mut nested = PackagePlan::inspect(root.path(), None)?;
    nested.resolve(root.path(), &nested.default_options(), &BTreeSet::new())?;
    assert_eq!(
        nested.files[0].destination,
        "DLC/DLC_MOD_X/CookedPCConsole/a/New.pcc"
    );
    Ok(())
}

// @variants: both
#[test]
fn tlk_activation_is_only_valid_for_le1_folder_alternates() -> Result<()> {
    let text = "((FriendlyName=Text,Condition=COND_MANUAL,ModOperation=OP_ENABLE_TLKMERGE_OPTIONKEY,LE1TLKOptionKey=Chosen))";
    assert_eq!(
        Alternate::parse(Some(text), true, Target::Le1)?[0].tlk_key(),
        Some("Chosen")
    );
    assert!(Alternate::parse(Some(text), false, Target::Le1).is_err());
    for target in [Target::Le2, Target::Le3] {
        assert!(Alternate::parse(Some(text), true, target).is_err());
    }
    Ok(())
}

// @variants: both
#[test]
fn file_size_conditions_validate_paths_and_require_known_results() -> Result<()> {
    let text = "((FriendlyName=Sizes,Condition=COND_SPECIFIC_SIZED_FILES,RequiredFileRelativePaths=BioGame/CookedPCConsole/Engine.pcc;Binaries/Win64/Game.exe,RequiredFileSizes=9;0,ModOperation=OP_NOTHING))";
    let option = Alternate::parse(Some(text), true, Target::Le1)?.remove(0);
    let mut sizes = BTreeMap::from([
        ("biogame/cookedpcconsole/engine.pcc".into(), Some(9)),
        ("binaries/win64/game.exe".into(), Some(0)),
    ]);
    assert!(option.active_with_sizes(&BTreeSet::new(), &BTreeSet::new(), &sizes)?);
    sizes.insert("biogame/cookedpcconsole/engine.pcc".into(), Some(8));
    assert!(!option.active_with_sizes(&BTreeSet::new(), &BTreeSet::new(), &sizes)?);
    sizes.insert("biogame/cookedpcconsole/engine.pcc".into(), None);
    assert!(
        option
            .active_with_sizes(&BTreeSet::new(), &BTreeSet::new(), &sizes)
            .is_err()
    );
    for invalid in [
        text.replace("9;0", "9"),
        text.replace("9;0", "-1;0"),
        text.replace("9;0", "9223372036854775808;0"),
        text.replace("BioGame/CookedPCConsole/Engine.pcc", "../outside"),
        text.replace("BioGame/CookedPCConsole/Engine.pcc", "/absolute"),
        text.replace(
            "Binaries/Win64/Game.exe",
            "biogame/cookedpcconsole/engine.pcc",
        ),
    ] {
        assert!(Alternate::parse(Some(&invalid), true, Target::Le1).is_err());
    }
    assert!(Alternate::parse(Some(text), false, Target::Le1).is_err());
    Ok(())
}

// @variants: both
#[test]
fn advanced_installer_metadata_validates_images_and_hidden_options() -> Result<()> {
    use std::fs;
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("Source/CookedPCConsole"))?;
    fs::create_dir_all(root.path().join("M3Images"))?;
    fs::write(
        root.path().join("Source/CookedPCConsole/Example.pcc"),
        b"example",
    )?;
    let picture = gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 4, 4)
        .context("missing test image")?;
    picture.fill(0x556677ff);
    fs::write(
        root.path().join("M3Images/Choice.png"),
        picture.save_to_bufferv("png", &[])?,
    )?;
    let text = "[ModManager]\ncmmver=9.2\nminbuild=137\nimportedby=137\n[ModInfo]\ngame=LE1\nmodname=Metadata\nmodver=1.0\nmoddev=Deployd\nmoddesc=Fixture\nsortalternates=true\n[BASEGAME]\njobdescription=Fixture\n[CUSTOMDLC]\nsourcedirs=Source\ndestdirs=DLC_MOD_Test\naltdlc=((FriendlyName=Visible,Condition=COND_MANUAL,ModOperation=OP_NOTHING,ImageAssetName=Choice.png,ImageHeight=64,SortIndex=1),(FriendlyName=Hidden,Hidden=true,CheckedByDefault=true,Condition=COND_MANUAL,ModOperation=OP_NOTHING))\n";
    let path = root.path().join("moddesc.ini");
    fs::write(&path, text)?;
    let plan = PackagePlan::inspect(root.path(), None)?;
    assert_eq!(plan.images.len(), 1);
    assert_eq!(plan.display_options().len(), 1);
    assert_eq!(plan.default_options().len(), 1);
    for invalid in [
        text.replace("minbuild=137", "minbuild=138"),
        text.replace("Hidden=true", "Hidden=true,OptionGroup=Group"),
        text.replace("Choice.png", "../Choice.png"),
    ] {
        fs::write(&path, invalid)?;
        assert!(PackagePlan::inspect(root.path(), None).is_err());
    }
    fs::write(&path, text)?;
    fs::write(root.path().join("M3Images/Choice.png"), b"invalid image")?;
    assert!(PackagePlan::inspect(root.path(), None).is_err());
    Ok(())
}
