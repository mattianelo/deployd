use super::*;

fn baseline() -> Baseline {
    let mut baseline = Baseline {
        game_id: Target::Le1.game_id().into(),
        sha256: String::new(),
        files: vec![super::super::baseline::BaselineFile {
            relative: BINK.into(),
            size: 4,
            modified: 0,
            sha256: "a".repeat(64),
        }],
    };
    baseline.sha256 = "c".repeat(64);
    baseline
}

// @variants: both
#[test]
fn resolves_game_specific_runtime_dependencies_and_rejects_unknown_versions() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let selected = required(target);
        validate(&selected, target)?;
        assert_eq!(selected.len(), if target == Target::Le1 { 4 } else { 2 });
        let mut future = selected.clone();
        future[0].version = "future".into();
        assert!(validate(&future, target).is_err());
        assert!(validate(&selected[1..], target).is_err());
    }
    assert!(validate(&required(Target::Le1), Target::Le2).is_err());
    Ok(())
}

// @variants: both
#[test]
fn component_plans_preserve_game_owned_bink_and_reject_unmanaged_replacements() -> Result<()> {
    let mut baseline = baseline();
    let selected = required(Target::Le1);
    let plan = Plan::inspect(&selected, &baseline, Target::Le1)?;
    let original = plan
        .files
        .iter()
        .find(|f| f.relative == ORIGINAL)
        .context("missing backing")?;
    assert_eq!(original.sha256, baseline.files[0].sha256);
    assert_eq!(plan.files.len(), 7);
    baseline.files.push(super::super::baseline::BaselineFile {
        relative: "Binaries/Win64/msvcp140.dll".into(),
        size: 1,
        modified: 0,
        sha256: "b".repeat(64),
    });
    assert!(Plan::inspect(&selected, &baseline, Target::Le1).is_err());
    baseline.files.clear();
    assert!(Plan::inspect(&selected, &baseline, Target::Le1).is_err());
    Ok(())
}

// @variants: both
#[test]
fn runtime_artifacts_require_exact_sizes_and_hashes() -> Result<()> {
    use sha2::{Digest, Sha256};
    let bytes = b"runtime";
    let hash = format!("{:x}", Sha256::digest(bytes));
    artifacts::verify(bytes, bytes.len() as u64, &hash)?;
    assert!(artifacts::verify(bytes, 1, &hash).is_err());
    assert!(artifacts::verify(b"changed", bytes.len() as u64, &hash).is_err());
    assert!(artifacts::payloads(Component::VisualCpp, bytes, &Control::recovery()).is_err());
    Ok(())
}

// @variants: both
#[tokio::test]
async fn runtime_cache_reuses_verified_files_offline_and_preserves_corruption() -> Result<()> {
    let root = tempfile::tempdir()?;
    let bytes = b"runtime";
    let hash = "d92c6a81b2ff50096bcda80885427d1f59a25b5f483f7055523504925d16ab23";
    let path = root.path().join("mele-components/artifacts").join(hash);
    std::fs::create_dir_all(path.parent().context("missing parent")?)?;
    std::fs::write(&path, bytes)?;
    let spec = || catalog::Artifact {
        url: "https://invalid.invalid/runtime",
        size: 7,
        sha256: hash,
    };
    let result = artifacts::load(
        spec(),
        root.path().into(),
        Control::recovery(),
        Arc::new(|_, _| {}),
    )
    .await?;
    assert_eq!(result, bytes);
    std::fs::write(&path, b"changed")?;
    assert!(
        artifacts::load(
            spec(),
            root.path().into(),
            Control::recovery(),
            Arc::new(|_, _| {})
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(path)?, b"changed");
    Ok(())
}

// @variants: both
#[test]
fn repair_permits_missing_managed_components_but_preserves_changed_files() -> Result<()> {
    let root = tempfile::tempdir()?;
    std::fs::create_dir_all(root.path().join("Binaries/Win64"))?;
    let baseline = baseline();
    let selected = required(Target::Le1);
    let plan = Plan::inspect(&selected, &baseline, Target::Le1)?;
    let recipe = super::super::recipe::Recipe {
        version: 2,
        backend_version: 1,
        target: Target::Le1,
        helper_version: None,
        language: "INT".into(),
        packages: Vec::new(),
        launcher: Vec::new(),
        components: selected,
    };
    let previous = State {
        removals: Default::default(),
        version: 2,
        generation: uuid::Uuid::new_v4().to_string(),
        profile: "profile".into(),
        files: plan.files.clone(),
        recipe: Some(recipe),
    };
    assert!(
        preflight(
            root.path(),
            &plan,
            Some(&previous),
            &baseline,
            Target::Le1,
            false,
            &Control::recovery()
        )
        .is_err()
    );
    preflight(
        root.path(),
        &plan,
        Some(&previous),
        &baseline,
        Target::Le1,
        true,
        &Control::recovery(),
    )?;
    let asi = root.path().join("Binaries/Win64/ASI");
    std::fs::create_dir_all(&asi)?;
    let unmanaged = asi.join("unknown.asi");
    std::fs::write(&unmanaged, b"unmanaged")?;
    assert!(
        preflight(
            root.path(),
            &plan,
            Some(&previous),
            &baseline,
            Target::Le1,
            true,
            &Control::recovery()
        )
        .is_err()
    );
    std::fs::remove_file(&unmanaged)?;
    std::os::unix::fs::symlink(root.path(), &unmanaged)?;
    assert!(
        preflight(
            root.path(),
            &plan,
            Some(&previous),
            &baseline,
            Target::Le1,
            true,
            &Control::recovery()
        )
        .is_err()
    );
    std::fs::remove_file(&unmanaged)?;
    let socket = std::os::unix::net::UnixListener::bind(&unmanaged)?;
    assert!(
        preflight(
            root.path(),
            &plan,
            Some(&previous),
            &baseline,
            Target::Le1,
            true,
            &Control::recovery()
        )
        .is_err()
    );
    drop(socket);
    std::fs::remove_file(&unmanaged)?;
    std::fs::write(root.path().join(BINK), b"modified")?;
    assert!(
        preflight(
            root.path(),
            &plan,
            Some(&previous),
            &baseline,
            Target::Le1,
            true,
            &Control::recovery()
        )
        .is_err()
    );
    assert_eq!(std::fs::read(root.path().join(BINK))?, b"modified");
    Ok(())
}

// @variants: both
#[test]
fn texture_components_follow_the_game_and_require_their_loader_and_runtime() -> Result<()> {
    for target in [Target::Le1, Target::Le2, Target::Le3] {
        let mut selections = Vec::new();
        texture_runtime(&mut selections, target, true);
        validate(&selections, target)?;
        let textures: Vec<_> = selections
            .iter()
            .filter(|selection| selection.component.texture_target().is_some())
            .collect();
        assert_eq!(textures.len(), 1);
        assert_eq!(textures[0].component.texture_target(), Some(target));
        for dependency in [Component::BinkProxy, Component::VisualCpp] {
            let missing: Vec<_> = selections
                .iter()
                .filter(|selection| selection.component != dependency)
                .cloned()
                .collect();
            assert!(validate(&missing, target).is_err());
        }
        let other = if target == Target::Le1 {
            Target::Le2
        } else {
            Target::Le1
        };
        assert!(validate(&selections, other).is_err());
        texture_runtime(&mut selections, target, false);
        assert!(
            selections
                .iter()
                .all(|selection| selection.component.texture_target().is_none())
        );
    }
    Ok(())
}
