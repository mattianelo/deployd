use std::fs;

use sha2::{Digest, Sha256};

use super::*;

// @variants: both
#[test]
fn references_do_not_copy_until_needed_and_writes_are_independent() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let candidate = temp.path().join("candidate");
    fs::create_dir(&source)?;
    fs::create_dir(&candidate)?;
    fs::write(source.join("file"), b"original")?;
    let expected = Identity {
        size: 8,
        sha256: format!("{:x}", Sha256::digest(b"original")),
    };
    let mut sources: Sources = candidate.clone().into();
    let control = Control::recovery();
    sources.insert(
        "BioGame/file".into(),
        source.clone(),
        "file".into(),
        expected.clone(),
        &control,
    )?;
    assert!(!candidate.join("BioGame/file").exists());
    sources.verify("BioGame/file", Some(&expected), &control)?;
    assert_eq!(
        sources.subtree("BioGame").resolve("file"),
        source.join("file")
    );
    sources.materialize("BioGame/file", &control)?;
    assert_eq!(fs::read(candidate.join("BioGame/file"))?, b"original");
    fs::write(candidate.join("BioGame/file"), b"modified")?;
    assert_eq!(fs::read(source.join("file"))?, b"original");
    assert!(
        sources
            .verify("BioGame/file", Some(&expected), &control)
            .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn changed_referenced_sources_and_unexpected_candidate_files_are_rejected() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("candidate");
    fs::create_dir(&root)?;
    fs::write(temp.path().join("source"), b"old")?;
    let expected = Identity {
        size: 3,
        sha256: format!("{:x}", Sha256::digest(b"old")),
    };
    let mut sources: Sources = root.clone().into();
    let control = Control::recovery();
    sources.insert(
        "file".into(),
        temp.path().into(),
        "source".into(),
        expected.clone(),
        &control,
    )?;
    fs::write(root.join("file"), b"old")?;
    assert!(sources.verify("file", Some(&expected), &control).is_err());
    fs::remove_file(root.join("file"))?;
    fs::write(temp.path().join("source"), b"new")?;
    assert!(sources.materialize("file", &control).is_err());
    assert!(!root.join("file").exists());
    Ok(())
}

// @variants: both
#[test]
fn references_preserve_directory_case_and_reject_overlapping_paths() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("candidate");
    fs::create_dir(&root)?;
    fs::write(temp.path().join("source"), b"old")?;
    let expected = Identity {
        size: 3,
        sha256: format!("{:x}", Sha256::digest(b"old")),
    };
    let control = Control::recovery();
    let mut sources: Sources = root.into();
    sources.insert(
        "BioGame/file".into(),
        temp.path().into(),
        "source".into(),
        expected.clone(),
        &control,
    )?;
    for path in [
        "biogame/other",
        "BioGame/File",
        "BioGame/file/child",
        "BioGame",
    ] {
        assert!(
            sources
                .insert(
                    path.into(),
                    temp.path().into(),
                    "source".into(),
                    expected.clone(),
                    &control
                )
                .is_err()
        );
    }
    sources.remove("BioGame/file", Some(&expected), &control)?;
    assert!(
        sources
            .insert(
                "biogame/other".into(),
                temp.path().into(),
                "source".into(),
                expected.clone(),
                &control
            )
            .is_err()
    );
    sources.insert(
        "BioGame/other".into(),
        temp.path().into(),
        "source".into(),
        expected,
        &control,
    )?;
    Ok(())
}
