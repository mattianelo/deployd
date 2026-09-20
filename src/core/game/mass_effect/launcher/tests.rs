use std::fs;

use super::*;

// @variants: both
#[test]
fn launcher_layouts_preserve_scope_and_require_explicit_consent() -> Result<()> {
    let source = tempfile::tempdir()?;
    fs::create_dir_all(source.path().join("Game/Launcher/Content"))?;
    fs::write(
        source.path().join("Game/Launcher/Content/Intro.bik"),
        b"video",
    )?;
    let mut entry = parse(source.path(), "Intro")?;
    entry.id = uuid::Uuid::new_v4().to_string();
    entry.owner = "mass-effect-le1".into();
    assert_eq!(entry.files[0].destination, "Content/Intro.bik");
    assert!(validate_entries(&[entry.clone()]).is_err());
    entry.approval = entry.source_sha256.clone();
    validate_entries(&[entry.clone()])?;
    entry.files[0].destination = "../ME1/BioGame/Test.pcc".into();
    assert!(validate_entries(&[entry]).is_err());
    for path in [
        "MassEffectLauncher.exe",
        "BINK2W64.DLL",
        "bink2w64_original.dll",
        "../system/Test.dll",
        "~docs~/Test.ini",
        "Binaries/Win64/Test.dll",
        "Content/Test.dll",
        "ASI/Test.sh",
        "ASI/nested/Test.asi",
    ] {
        assert!(destination(path).is_err(), "{path}");
    }
    Ok(())
}

// @variants: both
#[test]
fn launcher_manifests_validate_targets_renamed_plugins_and_unknown_operations() -> Result<()> {
    let source = tempfile::tempdir()?;
    fs::write(
        source.path().join("plugin.dat"),
        super::super::binary::tests::plugin(),
    )?;
    let text = "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LELAUNCHER\nmodname=Plugin\n[BASEGAME]\nmoddir=.\nnewfiles=plugin.dat\nreplacefiles=ASI\\Plugin.asi\n";
    fs::write(source.path().join("moddesc.ini"), text)?;
    assert_eq!(
        parse(source.path(), "ignored")?.files[0].destination,
        "ASI/Plugin.asi"
    );
    fs::write(
        source.path().join("moddesc.ini"),
        text.replace("LELAUNCHER", "LE2"),
    )?;
    assert!(parse(source.path(), "ignored").is_err());
    fs::write(
        source.path().join("moddesc.ini"),
        format!("{text}unknownjob=true\n"),
    )?;
    assert!(parse(source.path(), "ignored").is_err());
    fs::write(source.path().join("moddesc.ini"), text)?;
    fs::write(source.path().join("plugin.dat"), b"invalid")?;
    assert!(parse(source.path(), "ignored").is_err());
    Ok(())
}

// @variants: both
#[test]
fn launcher_sources_reject_links_case_collisions_and_changed_cached_inputs() -> Result<()> {
    let source = tempfile::tempdir()?;
    fs::create_dir_all(source.path().join("Content"))?;
    fs::write(source.path().join("Content/Test.swf"), b"movie")?;
    fs::write(source.path().join("Content/test.swf"), b"other")?;
    assert!(parse(source.path(), "movie").is_err());
    fs::remove_file(source.path().join("Content/test.swf"))?;
    let entry = parse(source.path(), "movie")?;
    fs::write(source.path().join("Content/Test.swf"), b"modified")?;
    assert!(verify_source(source.path(), &entry, &Control::recovery()).is_err());
    std::os::unix::fs::symlink("Test.swf", source.path().join("Content/Other.swf"))?;
    assert!(parse(source.path(), "movie").is_err());
    Ok(())
}

// @variants: both
#[test]
fn structured_launcher_manifests_expand_declared_folders() -> Result<()> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("Payload/Videos"))?;
    fs::write(root.path().join("Payload/Videos/Intro.bik"), b"video")?;
    fs::write(
        root.path().join("moddesc.ini"),
        "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LELAUNCHER\nmodname=Intro\n[BASEGAME]\nmoddir=Payload\ngamedirectorystructure=true\nnewfiles=Videos\nreplacefiles=Content\n",
    )?;
    let entry = parse(root.path(), "ignored")?;
    assert_eq!(entry.files.len(), 1);
    assert_eq!(entry.files[0].source, "Payload/Videos/Intro.bik");
    assert_eq!(entry.files[0].destination, "Content/Intro.bik");
    Ok(())
}

// @variants: both
#[test]
fn launcher_import_selects_its_component_from_a_game_bundle() -> Result<()> {
    let root = tempfile::tempdir()?;
    let game = root.path().join("LE2/Game Mod");
    fs::create_dir_all(&game)?;
    fs::write(
        game.join("moddesc.ini"),
        "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE2\nmodname=Game\n",
    )?;
    let launcher = root.path().join("LELauncher/Launcher Mod");
    fs::create_dir_all(launcher.join("LELAUNCHER"))?;
    fs::write(
        launcher.join("moddesc.ini"),
        "[ModManager]\ncmmver=8\n[ModInfo]\ngame=LELAUNCHER\nmodname=Launcher\n[LELAUNCHER]\nmoddir=LELAUNCHER\n",
    )?;
    fs::write(launcher.join("LELAUNCHER/ME2.bik"), b"launcher")?;

    let bundled = inspect_bundle(root.path())?.context("missing launcher component")?;
    assert_eq!(bundled.entry.name, "Launcher");
    assert_eq!(bundled.entry.files[0].destination, "ME2.bik");
    assert_eq!(bundled.source, Path::new("LELauncher/Launcher Mod"));
    assert!(
        bundled
            .entry
            .sources
            .iter()
            .all(|source| !source.relative.starts_with("LELauncher/"))
    );
    Ok(())
}

// @variants: both
#[test]
#[ignore = "requires the maintainer-supplied Unofficial LE2 Patch directory"]
fn imports_launcher_component_from_supplied_le2_patch_bundle() -> Result<()> {
    let root = Path::new("modTesting/Unofficial Mass Effect 2 Legendary Edition Patch");
    let bundled = inspect_bundle(root)?.context("missing launcher component")?;
    assert_eq!(
        bundled.entry.name,
        "Unofficial LE2 Patch Launcher Video Fix"
    );
    assert_eq!(bundled.entry.files.len(), 1);
    assert_eq!(bundled.entry.files[0].destination, "ME2.bik");
    assert!(
        bundled
            .entry
            .sources
            .iter()
            .all(|source| !source.relative.starts_with("LELauncher/"))
    );
    Ok(())
}
