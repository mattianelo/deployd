use std::fs;
use std::os::unix::fs::PermissionsExt;

use anyhow::Result;
use serde_json::json;

use super::*;

fn fixture(root: &Path, game: u8) -> Result<(Store, Vec<Source>, Record)> {
    let input = root.join("input");
    fs::create_dir_all(input.join("DLC_MOD_Test/CookedPCConsole"))?;
    fs::write(
        input.join("DLC_MOD_Test/CookedPCConsole/Test.pcc"),
        b"texture",
    )?;
    fs::write(
        input.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.1\n[ModInfo]\ngame=LE{game}\nmodname=Example\nmodver=1.0\nmoddev=Author\nmoddesc=Content\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_Test\ndestdirs=DLC_MOD_Test\n"
        ),
    )?;
    let plan = PackagePlan::inspect(&input, None)?;
    let id = uuid::Uuid::new_v4().to_string();
    let record = serde_json::from_value(json!({
        "version": 1, "target": format!("LE{game}"),
        "package": {"id": id, "source_sha256": plan.source_sha256,
            "archive_sha256": null, "manifest_version": "9.1",
            "mod_version": "1.0", "enabled": false, "options": []}
    }))?;
    let store = Store::create(root, "game", "store")?;
    let mut sources = Vec::new();
    for directory in ["", "/DLC_MOD_Test", "/DLC_MOD_Test/CookedPCConsole"] {
        sources.push(Source {
            path: format!("cache/{id}{directory}"),
            content: None,
            mode: 0o755,
        });
    }
    for file in plan.sources {
        sources.push(Source {
            path: format!("cache/{id}/{}", file.relative),
            content: Some(store.retain(&input.join(file.relative), &Control::default())?),
            mode: 0o644,
        });
    }
    Ok((store, sources, record))
}

// @variants: both
#[test]
fn unchanged_disabled_packages_remain_retained_without_materialization() -> Result<()> {
    for game in 1..=3 {
        let temp = tempfile::tempdir()?;
        let (store, sources, mut record) = fixture(temp.path(), game)?;
        let expected = record.clone();
        let candidate = temp.path().join("candidate");
        prepare_package(
            &store,
            &candidate,
            &sources,
            &mut record,
            &Control::default(),
        )?;
        assert!(!candidate.exists());
        assert_eq!(record, expected);
        for source in &sources {
            if let Some(identity) = &source.content {
                store.verify(identity, &Control::default())?;
            }
        }
        record.package.enabled = true;
        prepare_package(
            &store,
            &candidate,
            &sources,
            &mut record,
            &Control::default(),
        )?;
        assert_eq!(
            fs::read(
                candidate
                    .join("mele-sources")
                    .join(&record.package.source_sha256)
                    .join("DLC_MOD_Test/CookedPCConsole/Test.pcc")
            )?,
            b"texture"
        );
    }
    Ok(())
}

// @variants: both
#[test]
fn edited_disabled_packages_are_inspected_and_record_the_new_version() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, mut sources, mut record) = fixture(temp.path(), 2)?;
    let old_hash = record.package.source_sha256.clone();
    let manifest = temp.path().join("input/moddesc.ini");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest)?.replace("modver=1.0", "modver=2.0"),
    )?;
    let source = sources
        .iter_mut()
        .find(|source| source.path.ends_with("/moddesc.ini"))
        .expect("manifest source");
    source.content = Some(store.retain(&manifest, &Control::default())?);
    prepare_package(
        &store,
        &temp.path().join("candidate"),
        &sources,
        &mut record,
        &Control::default(),
    )?;
    assert!(!record.package.enabled);
    assert_eq!(record.package.mod_version, "2.0");
    assert_ne!(record.package.source_sha256, old_hash);
    Ok(())
}

// @variants: both
#[test]
fn corrupt_disabled_content_still_blocks_preparation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, sources, mut record) = fixture(temp.path(), 2)?;
    let identity = sources
        .iter()
        .find_map(|source| source.content.as_ref())
        .expect("retained file");
    let object = store.source(identity)?;
    fs::set_permissions(&object, fs::Permissions::from_mode(0o600))?;
    fs::write(&object, b"damaged")?;
    let candidate = temp.path().join("candidate");
    assert!(
        prepare_package(
            &store,
            &candidate,
            &sources,
            &mut record,
            &Control::default()
        )
        .is_err()
    );
    assert!(!candidate.exists());
    Ok(())
}

// @variants: both
#[test]
fn invalid_edits_to_disabled_packages_are_not_skipped() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (store, mut sources, mut record) = fixture(temp.path(), 2)?;
    let manifest = temp.path().join("input/moddesc.ini");
    fs::write(&manifest, b"invalid manifest")?;
    let source = sources
        .iter_mut()
        .find(|source| source.path.ends_with("/moddesc.ini"))
        .expect("manifest source");
    source.content = Some(store.retain(&manifest, &Control::default())?);
    assert!(
        prepare_package(
            &store,
            &temp.path().join("candidate"),
            &sources,
            &mut record,
            &Control::default()
        )
        .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn preview_reuse_requires_the_complete_matching_package_sources() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (_, sources, record) = fixture(temp.path(), 1)?;
    assert!(matches_package(&sources, &record.package));
    let file = sources
        .iter()
        .position(|source| source.content.is_some())
        .expect("Source file");
    for change in 0..4 {
        let mut edited = sources.clone();
        match change {
            0 => {
                edited.remove(file);
            }
            1 => {
                edited[file].content.as_mut().expect("File identity").sha256 = "0".repeat(64);
            }
            2 => {
                edited[file].path.push_str(".renamed");
            }
            _ => {
                let mut added = edited[file].clone();
                added.path.push_str(".added");
                edited.push(added);
            }
        }
        assert!(!matches_package(&edited, &record.package));
    }
    assert!(!matches_package(&[], &record.package));
    Ok(())
}
