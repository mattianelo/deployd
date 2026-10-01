use super::super::protocol::TlkChange;
use super::*;

fn job() -> Job {
    Job::Tlk {
        targets: vec![FileIdentity {
            path: "CookedPCConsole/file.pcc".into(),
            size: 3,
            sha256: digest(b"old"),
        }],
        changes: vec![TlkChange {
            target: "CookedPCConsole/file.pcc".into(),
            export: "Example.tlk".into(),
            strings: vec![],
        }],
    }
}

fn output(parent: &Path) -> Result<ValidatedOutput> {
    let stage = tempfile::tempdir_in(parent)?;
    fs::create_dir_all(stage.path().join("output/CookedPCConsole"))?;
    fs::write(stage.path().join("output/CookedPCConsole/file.pcc"), b"new")?;
    Ok(ValidatedOutput {
        storage: super::super::OutputStorage::Temporary(stage),
        cache_key: None,
        files: vec![FileIdentity {
            path: "CookedPCConsole/file.pcc".into(),
            size: 3,
            sha256: digest(b"new"),
        }],
    })
}

// @variants: both
#[test]
fn cache_survives_reopening_and_rejects_damage_or_partial_publication() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ticket = Ticket {
        root: temp.path().join("cache"),
        key: digest(b"job"),
    };
    let control = Control::recovery();
    let job = job();
    assert!(ticket.load(&job, &control).is_err());
    let cached = ticket.publish(&output(temp.path())?, &job, &control)?;
    assert_eq!(
        fs::read(cached.root().join("CookedPCConsole/file.pcc"))?,
        b"new"
    );
    drop(cached);
    crate::utils::verified_files::invalidate()?;
    let reopened = Ticket {
        root: ticket.root.clone(),
        key: ticket.key.clone(),
    };
    drop(reopened.load(&job, &control)?);
    fs::write(
        ticket.directory().join("output/CookedPCConsole/file.pcc"),
        b"bad",
    )?;
    assert!(ticket.load(&job, &control).is_err());
    drop(ticket.publish(&output(temp.path())?, &job, &control)?);
    fs::remove_file(ticket.directory().join("manifest.json"))?;
    assert!(ticket.load(&job, &control).is_err());
    control
        .cancelled
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(
        ticket
            .publish(&output(temp.path())?, &job, &control)
            .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn job_inputs_order_and_backend_dependencies_invalidate_results() -> Result<()> {
    let mut job = job();
    let mut dependencies = BTreeMap::from([("codec".into(), "first".into())]);
    let original = fingerprint(&job, &dependencies)?;
    dependencies.insert("codec".into(), "updated".into());
    assert_ne!(original, fingerprint(&job, &dependencies)?);
    dependencies.insert("codec".into(), "first".into());
    if let Job::Tlk { targets, changes } = &mut job {
        targets[0].sha256 = digest(b"upstream");
        changes[0].export = "Other.tlk".into();
    }
    assert_ne!(original, fingerprint(&job, &dependencies)?);
    Ok(())
}

// @variants: both
#[test]
fn cache_write_failure_keeps_original_output_available() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let blocked = temp.path().join("blocked");
    fs::write(&blocked, b"not a directory")?;
    let ticket = Ticket {
        root: blocked,
        key: digest(b"job"),
    };
    let output = output(temp.path())?;
    assert!(
        ticket
            .publish(&output, &job(), &Control::recovery())
            .is_err()
    );
    assert_eq!(
        fs::read(output.root().join("CookedPCConsole/file.pcc"))?,
        b"new"
    );
    Ok(())
}

fn recipe(language: &str) -> Recipe {
    Recipe {
        version: 1,
        backend_version: 1,
        helper_version: Some(protocol::VERSION.into()),
        target: crate::core::game::mass_effect::Target::Le1,
        language: language.into(),
        packages: vec![],
        components: vec![],
        launcher: vec![],
    }
}

// @variants: both
#[test]
fn cleanup_keeps_two_successful_recipes_other_profiles_and_active_outputs() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = paths::mele_result_cache_in(temp.path());
    let tickets: Vec<_> = (0..4)
        .map(|n| Ticket {
            root: root.clone(),
            key: digest(&[n]),
        })
        .collect();
    let mut outputs = Vec::new();
    for ticket in &tickets {
        outputs.push(ticket.publish(&output(temp.path())?, &job(), &Control::recovery())?);
    }
    for (n, ticket) in tickets.iter().enumerate() {
        let recipe = recipe(&format!("LANG{n}"));
        prepared(temp.path(), &recipe, BTreeSet::from([ticket.key.clone()]))?;
        committed(
            temp.path(),
            &recipe,
            if n == 3 { "other" } else { "profile" },
        )?;
    }
    assert!(tickets[0].directory().exists());
    drop(outputs);
    committed(temp.path(), &recipe("LANG2"), "profile")?;
    assert!(!tickets[0].directory().exists());
    for ticket in &tickets[1..] {
        assert!(ticket.directory().exists());
    }
    Ok(())
}
