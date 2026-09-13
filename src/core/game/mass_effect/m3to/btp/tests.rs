use std::fs;

use super::*;
use crate::core::game::mass_effect::operation::Control;
use crate::core::game::mass_effect::package::PackagePlan;

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_name(bytes: &mut [u8], value: &str) {
    for (slot, word) in bytes.chunks_exact_mut(2).zip(value.encode_utf16()) {
        slot.copy_from_slice(&word.to_le_bytes());
    }
}

fn fixture(root: &Path, target: Target, external: bool) -> Result<()> {
    let (game, version, licensee, group) = match target {
        Target::Le1 => ("LE1", 684, 171, 88),
        Target::Le2 => ("LE2", 684, 168, 89),
        Target::Le3 => ("LE3", 685, 205, 87),
    };
    fs::create_dir_all(root.join("DLC_MOD_Test/CookedPCConsole"))?;
    fs::write(
        root.join("moddesc.ini"),
        format!(
            "[ModManager]\ncmmver=9.2\n[ModInfo]\ngame={game}\nmodname=Texture fixture\nmodver=1\nmoddev=Deployd\nmoddesc=Fixture\n[CUSTOMDLC]\nsourcedirs=DLC_MOD_Test\ndestdirs=DLC_MOD_Test\n[UPDATES]\nnexusupdatecheck=false\n[ASIMODS]\nasimodstoinstall=((GroupID={group}))\n"
        ),
    )?;
    if target == Target::Le1 {
        fs::write(
            root.join("DLC_MOD_Test/AutoLoad.ini"),
            "[ME1DLCMOUNT]\nModMount=300\n",
        )?;
    } else {
        let mut mount = vec![0; 44];
        let offset = if target == Target::Le2 { 0 } else { 4 };
        put32(&mut mount, offset, version);
        put32(&mut mount, offset + 4, licensee);
        put32(
            &mut mount,
            offset + 8,
            if target == Target::Le2 { 65643 } else { 196715 },
        );
        put32(&mut mount, offset + 12, 300);
        fs::write(root.join("DLC_MOD_Test/CookedPCConsole/Mount.dlc"), mount)?;
    }
    let mut metadata = vec![0xc1, 0x83, 0x2a, 0x9e, 0, 0, 0, 0];
    put32(&mut metadata, 4, version | licensee << 16);
    fs::write(root.join("DLC_MOD_Test/BTPMetadata.btm"), &metadata)?;
    let tfc_offset = 48 + 836 + 64;
    let mut package = vec![0; tfc_offset + if external { 288 } else { 144 }];
    package[..6].copy_from_slice(b"LETEXM");
    package[6] = 2;
    put32(&mut package, 8, target_hash(target, "DLC_MOD_Test"));
    put32(&mut package, 12, 1);
    put32(&mut package, 16, if external { 2 } else { 1 });
    put32(&mut package, 20, tfc_offset as u32);
    put32(&mut package, 28, crc32fast::hash(&metadata));
    let texture = &mut package[48..884];
    put_name(&mut texture[..512], "Example.Texture");
    put32(texture, 512, u32::from(external));
    put32(texture, 516, 2);
    texture[523] = 1;
    put32(texture, 524, if external { 256 } else { 1 });
    put32(texture, 528, 64);
    put32(texture, 532, if external { 16 } else { 884 });
    texture[540] = 1;
    texture[542] = 1;
    put32(texture, 544, if external { 4 } else { 0 });
    put_name(&mut package[tfc_offset..tfc_offset + 128], "None");
    if external {
        put_name(
            &mut package[tfc_offset + 144..tfc_offset + 272],
            "Textures_DLC_MOD_Test",
        );
        package[tfc_offset + 272..].fill(7);
        let mut tfc = vec![0; 80];
        tfc[..16].fill(7);
        fs::write(
            root.join("DLC_MOD_Test/CookedPCConsole/Textures_DLC_MOD_Test.tfc"),
            tfc,
        )?;
    }
    fs::write(
        root.join("DLC_MOD_Test/CombinedTextureOverrides.btp"),
        package,
    )?;
    Ok(())
}

// @variants: both
#[test]
fn accepts_precompiled_overrides_and_revalidates_the_combined_staging_tree() -> Result<()> {
    for game in [Target::Le1, Target::Le2, Target::Le3] {
        for external in [false, true] {
            let temp = tempfile::tempdir()?;
            fixture(temp.path(), game, external)?;
            let plan = PackagePlan::inspect(temp.path(), Some(game))?;
            assert!(plan.manifest.texture_runtime);
            assert_eq!(plan.m3to.len(), 1);
            assert!(
                plan.required_transformations()
                    .iter()
                    .all(|kind| kind.supported())
            );
            let staged = tempfile::tempdir()?;
            let mut files = Vec::new();
            for mapping in &plan.files {
                let destination = format!("BioGame/{}", mapping.destination);
                let path = staged.path().join(&destination);
                fs::create_dir_all(path.parent().context("missing parent")?)?;
                fs::copy(temp.path().join(&mapping.source), path)?;
                let mut file = plan
                    .sources
                    .iter()
                    .find(|file| file.relative == mapping.source)
                    .context("missing source")?
                    .clone();
                file.relative = destination;
                files.push(file);
            }
            super::super::validate_staged(staged.path(), &files, game, &Control::recovery())?;
            let btm = staged
                .path()
                .join("BioGame/DLC/DLC_MOD_Test/BTPMetadata.btm");
            fs::write(btm, b"replaced")?;
            assert!(
                super::super::validate_staged(staged.path(), &files, game, &Control::recovery())
                    .is_err()
            );
        }
    }
    Ok(())
}

// @variants: both
#[test]
fn rejects_corrupted_pairs_foreign_targets_and_out_of_bounds_mips() -> Result<()> {
    let temp = tempfile::tempdir()?;
    for (offset, value) in [
        (6, 3),
        (8, 0),
        (28, 0),
        (16, 0),
        (532, 0),
        (528, u32::MAX),
        (544, 16),
        (560, 1),
    ] {
        fixture(temp.path(), Target::Le2, false)?;
        let path = temp
            .path()
            .join("DLC_MOD_Test/CombinedTextureOverrides.btp");
        let mut bytes = fs::read(&path)?;
        let offset = if offset >= 512 { offset + 48 } else { offset };
        put32(&mut bytes, offset, value);
        fs::write(path, bytes)?;
        assert!(
            PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err(),
            "offset {offset}"
        );
    }
    fixture(temp.path(), Target::Le2, true)?;
    fs::write(
        temp.path()
            .join("DLC_MOD_Test/CookedPCConsole/Textures_DLC_MOD_Test.tfc"),
        [0; 80],
    )?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    fixture(temp.path(), Target::Le2, false)?;
    fs::remove_file(temp.path().join("DLC_MOD_Test/BTPMetadata.btm"))?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    Ok(())
}

// @variants: both
#[test]
fn rejects_hidden_and_mixed_m3to_layouts_before_installing() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path(), Target::Le2, false)?;
    let source = temp.path().join("DLC_MOD_Test/CookedPCConsole/TO_Test.pcc");
    fs::write(&source, b"source")?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    fs::remove_file(source)?;
    fs::write(
        temp.path()
            .join("DLC_MOD_Test/CookedPCConsole/TextureOverride-Test.m3to"),
        b"{}",
    )?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    fs::remove_file(
        temp.path()
            .join("DLC_MOD_Test/CookedPCConsole/TextureOverride-Test.m3to"),
    )?;
    fs::rename(
        temp.path()
            .join("DLC_MOD_Test/CombinedTextureOverrides.btp"),
        temp.path().join("DLC_MOD_Test/Other.btp"),
    )?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    fs::remove_file(temp.path().join("moddesc.ini"))?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    Ok(())
}

// @variants: both
#[test]
#[ignore = "Reads only supplied ALOT/ISL metadata, BTP tables, and TFC GUIDs; never reads texture payloads"]
fn validates_supplied_alot_and_isl_with_bounded_reads() -> Result<()> {
    inspect_corpus(
        Target::Le2,
        [
            ("ALotofTextures(ALOT)forLE2_2021.1", "DLC_MOD_ALOT"),
            ("ImprovedStaticLightingforLE2_2021.1.0", "DLC_MOD_ISL"),
        ],
    )
}

// @variants: both
#[test]
#[ignore = "Reads only supplied LE1 ALOT/ISL metadata, BTP tables, and TFC GUIDs; never reads texture payloads"]
fn validates_supplied_le1_alot_and_isl_with_bounded_reads() -> Result<()> {
    inspect_corpus(
        Target::Le1,
        [
            ("ALotofTextures(ALOT)forLE1_2021.1.3", "DLC_MOD_ALOT"),
            ("ImprovedStaticLightingforLE1_2021.1.0", "DLC_MOD_ISL"),
        ],
    )
}

fn inspect_corpus(target: Target, inputs: [(&str, &str); 2]) -> Result<()> {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("modTesting");
    for (folder, dlc) in inputs {
        let root = corpus.join(folder);
        let source = |relative: String, hash: bool| -> Result<SourceFile> {
            let path = root.join(&relative);
            let size = fs::metadata(&path)?.len();
            Ok(SourceFile {
                relative,
                size,
                sha256: if hash {
                    format!("{:x}", Sha256::digest(fs::read(path)?))
                } else {
                    String::new()
                },
            })
        };
        let manifest = fs::read_to_string(root.join("moddesc.ini"))?;
        let parsed = crate::core::game::mass_effect::manifest::Manifest::parse(&manifest)?;
        assert!(parsed.texture_runtime);
        assert_eq!(parsed.target, target);
        let mount_name = if target == Target::Le1 {
            "AutoLoad.ini"
        } else {
            "CookedPCConsole/Mount.dlc"
        };
        let mount = source(format!("{dlc}/{mount_name}"), true)?;
        assert_eq!(
            super::super::mount(&root, &mount, target)?,
            if dlc == "DLC_MOD_ALOT" { 23433 } else { 23434 }
        );
        let textures = inspect(
            &root,
            &source(format!("{dlc}/CombinedTextureOverrides.btp"), false)?,
            &source(format!("{dlc}/BTPMetadata.btm"), true)?,
            dlc,
            target,
            |name| source(format!("{dlc}/CookedPCConsole/{name}.tfc"), false),
            &Control::recovery(),
        )?;
        eprintln!("{folder}: {} texture entries validated", textures.len());
    }
    Ok(())
}

// @variants: both
#[test]
fn cancellation_stops_texture_validation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path(), Target::Le2, false)?;
    let plan = PackagePlan::inspect(temp.path(), Some(Target::Le2))?;
    let control = Control::recovery();
    control
        .cancelled
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(
        super::super::inspect_controlled(
            temp.path(),
            &plan.sources,
            &plan.files,
            Target::Le2,
            "9.2",
            &control
        )
        .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn texture_precedence_uses_dlc_mounts_and_rejects_ambiguous_ties() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path(), Target::Le2, false)?;
    let first = PackagePlan::inspect(temp.path(), Some(Target::Le2))?;
    for file in &first.files {
        let destination = file.source.replace("DLC_MOD_Test/", "DLC_MOD_Other/");
        let path = temp.path().join(destination);
        fs::create_dir_all(path.parent().context("missing parent")?)?;
        fs::copy(temp.path().join(&file.source), path)?;
    }
    let btp = temp
        .path()
        .join("DLC_MOD_Other/CombinedTextureOverrides.btp");
    let mut bytes = fs::read(&btp)?;
    put32(&mut bytes, 8, target_hash(Target::Le2, "DLC_MOD_Other"));
    fs::write(btp, bytes)?;
    let manifest = temp.path().join("moddesc.ini");
    let text = fs::read_to_string(&manifest)?
        .replace("dirs=DLC_MOD_Test", "dirs=DLC_MOD_Test;DLC_MOD_Other");
    fs::write(manifest, text)?;
    let error = PackagePlan::inspect(temp.path(), Some(Target::Le2))
        .err()
        .context("ambiguous mounts were accepted")?;
    assert!(error.to_string().contains("ambiguous DLC mount priority"));
    let mount = temp.path().join("DLC_MOD_Other/CookedPCConsole/Mount.dlc");
    let mut bytes = fs::read(&mount)?;
    put32(&mut bytes, 12, 301);
    fs::write(&mount, &bytes)?;
    let plan = PackagePlan::inspect(temp.path(), Some(Target::Le2))?;
    assert_eq!(plan.m3to.len(), 2);
    put32(&mut bytes, 4, 205);
    fs::write(mount, bytes)?;
    assert!(PackagePlan::inspect(temp.path(), Some(Target::Le2)).is_err());
    Ok(())
}

// @variants: both
#[test]
fn le1_mount_validation_matches_the_texture_runtimes_case_sensitive_reader() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path(), Target::Le1, false)?;
    let path = temp.path().join("DLC_MOD_Test/AutoLoad.ini");
    for text in [
        "[ME1DLCMOUNT]\nmodmount=300\n",
        "[Other]\nModMount=5\n[ME1DLCMOUNT]\nModMount=300\n",
    ] {
        fs::write(&path, text)?;
        assert!(PackagePlan::inspect(temp.path(), Some(Target::Le1)).is_err());
    }
    fs::write(path, "[ME1DLCMOUNT]\nModMount=+00300\n")?;
    PackagePlan::inspect(temp.path(), Some(Target::Le1))?;
    Ok(())
}
