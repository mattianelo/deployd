use anyhow::Result;

use super::{morph, save::Document};
use crate::core::game::mass_effect::Target;

fn fixtures() -> [(Target, &'static [u8]); 4] {
    [
        (
            Target::Le1,
            include_bytes!("../../../../../tests/fixtures/appearance/ME1LeSave.pcsav"),
        ),
        (
            Target::Le1,
            include_bytes!("../../../../../tests/fixtures/appearance/ME1LeExport.pcsav"),
        ),
        (
            Target::Le2,
            include_bytes!("../../../../../tests/fixtures/appearance/ME2LeSave.pcsav"),
        ),
        (
            Target::Le3,
            include_bytes!("../../../../../tests/fixtures/appearance/ME3Save.pcsav"),
        ),
    ]
}

// @variants: both
#[test]
fn preserves_saves_and_round_trips_hair_edits() -> Result<()> {
    for (target, bytes) in fixtures() {
        let mut doc = Document::read(bytes, target)?;
        assert_eq!(doc.write()?, bytes);
        let preset = morph::parse(
            include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph"),
            "example".into(),
        )?;
        if doc.morph.is_none() {
            doc.morph = Some(preset.morph);
        }
        let head = doc.morph.as_mut().unwrap();
        let vertices = head.lod0_vertices.clone();
        head.hair_mesh = "BIOG_Test.Hair.Test_MDL".into();
        let edited = doc.write()?;
        let reopened = Document::read(&edited, target)?;
        assert_eq!(reopened.morph.as_ref().unwrap().lod0_vertices, vertices);
        assert_eq!(reopened.morph, doc.morph);
        assert_eq!(reopened.write()?, edited);
        doc.reset();
        assert_eq!(doc.write()?, bytes);
    }
    Ok(())
}

#[test]
fn round_trips_accessories_and_materials_without_changing_face_shape() -> Result<()> {
    for (target, bytes) in fixtures() {
        let mut doc = Document::read(bytes, target)?;
        let Some(head) = doc.morph.as_mut() else {
            continue;
        };
        let mut expected = head.clone();
        expected.accessory_mesh.push("Test.Accessory.Mesh".into());
        expected
            .texture_parameters
            .push(("DeploydTexture".into(), "Test.Texture.Diffuse".into()));
        expected
            .scalar_parameters
            .push(("DeploydScalar".into(), 0.375));
        expected.vector_parameters.push((
            "DeploydColor".into(),
            morph::LinearColor(0.1, 0.2, 0.3, 1.0),
        ));
        *head = expected.clone();
        let reopened = Document::read(&doc.write()?, target)?;
        assert_eq!(reopened.morph, Some(expected));
    }
    Ok(())
}

#[test]
fn gibbed_presets_export_as_tse_ron_without_changing_values() -> Result<()> {
    for bytes in [
        include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph")
            .as_slice(),
        include_bytes!("../../../../../tests/fixtures/appearance/GibbedME3.me3headmorph")
            .as_slice(),
    ] {
        let preset = morph::parse(bytes, "preset".into())?;
        let text = preset.morph.export()?;
        let imported = morph::parse(text.as_bytes(), "renamed.headmorph".into())?;
        assert_eq!(imported.morph, preset.morph);
        assert!(imported.target.is_none());
        for end in [0, 24, 30, bytes.len() - 1] {
            assert!(morph::parse(&bytes[..end], "broken".into()).is_err());
        }
    }
    Ok(())
}

#[test]
fn rejects_damaged_saves_and_wrong_game_versions() {
    for (target, bytes) in fixtures() {
        let mut damaged = bytes.to_vec();
        damaged[15] ^= 0xff;
        assert!(Document::read(&damaged, target).is_err());
        assert!(Document::read(&bytes[..bytes.len() / 2], target).is_err());
        let wrong = if target == Target::Le1 {
            Target::Le2
        } else {
            Target::Le1
        };
        assert!(Document::read(bytes, wrong).is_err());
    }
}

#[test]
fn rejects_truncated_payloads_even_with_a_recomputed_checksum() {
    for (target, bytes) in fixtures()
        .into_iter()
        .filter(|(target, _)| *target != Target::Le1)
    {
        for missing in [1, 4, 16, 100] {
            let mut shortened = bytes[..bytes.len() - 4 - missing].to_vec();
            let checksum = super::binary::checksum(&shortened);
            super::binary::word(&mut shortened, checksum);
            assert!(Document::read(&shortened, target).is_err());
        }
    }
}

#[test]
fn rejects_invalid_presets_and_nonfinite_material_values() -> Result<()> {
    let preset = morph::parse(
        include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph"),
        "preset".into(),
    )?;
    let mut head = preset.morph;
    head.scalar_parameters
        .push(("InvalidValue".into(), f32::NAN));
    assert!(head.validate().is_err());
    head.scalar_parameters.pop();
    head.texture_parameters
        .push(("Texture".into(), "../../outside".into()));
    assert!(head.validate().is_err());
    head.texture_parameters.pop();
    head.scalar_parameters
        .extend([("Duplicate".into(), 1.0), ("Duplicate".into(), 2.0)]);
    assert!(head.validate().is_err());
    assert!(morph::parse(b"(setting: true)", "settings.ron".into()).is_err());
    let deep = format!("{}{}", "(".repeat(128), ")".repeat(128));
    assert!(morph::parse(deep.as_bytes(), "deep.ron".into()).is_err());
    Ok(())
}

#[test]
fn preserves_missing_headmorph_until_explicit_import() -> Result<()> {
    let mut doc = Document::read(fixtures()[2].1, Target::Le2)?;
    doc.morph = None;
    let bytes = doc.write()?;
    let mut empty = Document::read(&bytes, Target::Le2)?;
    assert!(empty.morph.is_none());
    assert!(!empty.changed());
    assert_eq!(empty.write()?, bytes);
    empty.morph = Some(
        morph::parse(
            include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph"),
            "preset".into(),
        )?
        .morph,
    );
    assert!(
        Document::read(&empty.write()?, Target::Le2)?
            .morph
            .is_some()
    );
    empty.reset();
    assert!(empty.morph.is_none());
    Ok(())
}

#[test]
fn rejects_excessive_collection_and_decompression_sizes() -> Result<()> {
    let mut preset =
        include_bytes!("../../../../../tests/fixtures/appearance/GibbedME2.me2headmorph").to_vec();
    let mut reader = super::binary::Reader::new(&preset[31..]);
    reader.string()?;
    let length = 31 + reader.position;
    preset[length..length + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(morph::parse(&preset, "oversized".into()).is_err());
    for offset in [4, 12] {
        let mut save = fixtures()[0].1.to_vec();
        save[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let end = save.len() - 12;
        let crc = super::binary::checksum(&save[..end]);
        save[end..end + 4].copy_from_slice(&crc.to_le_bytes());
        assert!(Document::read(&save, Target::Le1).is_err());
    }
    Ok(())
}
