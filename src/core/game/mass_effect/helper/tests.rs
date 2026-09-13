use std::os::unix::fs::{PermissionsExt, symlink};

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::core::location_recovery::activity_lock;

use super::*;
use protocol::{TlkChange, VERSION};

pub(super) fn control() -> Control {
    Control {
        cancelled: Arc::new(AtomicBool::new(false)),
        abandoned: Arc::new(AtomicBool::new(false)),
    }
}

fn identity(root: &Path, path: &str) -> Result<FileIdentity> {
    let bytes = fs::read(root.join(path))?;
    Ok(FileIdentity {
        path: path.into(),
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

fn capabilities() -> serde_json::Value {
    json!({"protocol":1,"backend":"deployd-mele","version":VERSION,"games":["LE1","LE2","LE3"],
        "capabilities":["le1-tlk","mele-plot","le1-m3da","le1-m3cd","mele-m3m-ordered"],"validation":["package-roundtrip"]})
}

struct Fixture {
    root: TempDir,
    backend: Backend,
    inputs: Inputs,
    parent: PathBuf,
    job: Job,
}

impl Fixture {
    fn new(body: &str) -> Result<Self> {
        let root = tempfile::tempdir()?;
        for directory in ["backend", "game", "candidate/CookedPCConsole", "staging"] {
            fs::create_dir_all(root.path().join(directory))?;
        }
        let inputs = Inputs {
            game: root.path().join("game"),
            candidate: root.path().join("candidate"),
            original: None,
        };
        let parent = root.path().join("staging");
        let backend = Backend {
            runtime: root.path().join("backend/runtime"),
            assembly: root.path().join("backend/Deployd.Mele.dll"),
            native_library: root.path().join("backend/libdeployd_oodle.so"),
        };
        let script = format!(
            "#!/usr/bin/python3\nimport hashlib, json, os, pathlib, sys, time\nif sys.argv[2] == 'capabilities':\n print({:?})\n sys.exit(0)\nrequest = json.loads(pathlib.Path(sys.argv[3]).read_text())\n{}\n",
            capabilities().to_string(),
            body
        );
        fs::write(&backend.runtime, script)?;
        fs::set_permissions(&backend.runtime, fs::Permissions::from_mode(0o700))?;
        fs::write(&backend.assembly, b"test assembly")?;
        fs::write(&backend.native_library, b"test library")?;
        let target = "CookedPCConsole/Example.pcc";
        fs::write(inputs.candidate.join(target), b"original")?;
        let job = Job::Tlk {
            targets: vec![identity(&inputs.candidate, target)?],
            changes: vec![TlkChange {
                target: target.into(),
                export: "Example.tlk".into(),
                strings: vec![],
            }],
        };
        Ok(Self {
            root,
            backend,
            inputs,
            parent,
            job,
        })
    }

    async fn run(&self) -> Result<ValidatedOutput> {
        transform(
            self.backend.clone(),
            self.inputs.clone(),
            self.parent.clone(),
            self.job.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_, _| {}),
        )
        .await
    }

    fn empty(&self) -> Result<()> {
        assert_eq!(fs::read_dir(&self.parent)?.count(), 0);
        assert_eq!(
            fs::read(self.inputs.candidate.join("CookedPCConsole/Example.pcc"))?,
            b"original"
        );
        Ok(())
    }
}

const SUCCESS: &str = r#"
output = pathlib.Path(request['output_root']) / request['targets'][0]['path']
output.parent.mkdir(parents=True)
output.write_bytes(b'transformed')
print(json.dumps(dict(protocol=1, type='progress', completed=1, total=1)), flush=True)
print(json.dumps(dict(protocol=1, type='complete', outputs=[dict(path=request['targets'][0]['path'], size=11, sha256=hashlib.sha256(b'transformed').hexdigest())])), flush=True)
"#;

// @variants: both
#[tokio::test]
async fn accepts_only_verified_outputs_and_preserves_input_copies() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let output = fixture.run().await?;
    assert_eq!(output.files.len(), 1);
    assert_eq!(
        fs::read(output.root().join(&output.files[0].path))?,
        b"transformed"
    );
    drop(output);
    fixture.empty()?;
    Ok(())
}

// @variants: both
#[tokio::test]
async fn discards_wrong_hashes_extra_files_and_failed_publication() -> Result<()> {
    for suffix in [
        "output.write_bytes(b'corrupted!!')",
        "(output.parent / 'unexpected.pcc').write_bytes(b'extra')",
        "(output.parent / 'extra-directory').mkdir()",
        "sys.exit(1)",
        "print('unstructured error', file=sys.stderr)",
        "print(json.dumps(dict(protocol=1,type='complete',outputs=[])))",
    ] {
        let fixture = Fixture::new(&format!("{SUCCESS}\n{suffix}"))?;
        assert!(fixture.run().await.is_err(), "{suffix}");
        fixture.empty()?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn surfaces_structured_helper_failures_without_publishing() -> Result<()> {
    let fixture = Fixture::new(
        "print(json.dumps(dict(protocol=1,type='error',code='InvalidDataException',message='Missing TLK export',at=[])),file=sys.stderr)\nsys.exit(1)",
    )?;
    let error = fixture.run().await.err().context("Expected failure")?;
    assert!(
        error.to_string().contains("Missing TLK export"),
        "{error:#}"
    );
    fixture.empty()
}

// @variants: both
#[tokio::test]
async fn kills_and_reaps_helpers_on_cancellation_and_timeout() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let pid = temp.path().join("pid");
    for cancel in [true, false] {
        let control = control();
        let mut command = Command::new("/usr/bin/python3");
        command.args(["-c", "import os,pathlib,sys,time; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); print('ready',flush=True); time.sleep(60)"])
            .arg(&pid).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        let cancelled = control.cancelled.clone();
        let stop = async {
            if cancel {
                let ready = tokio::time::timeout(Duration::from_secs(5), async {
                    while !pid.exists() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await;
                cancelled.store(true, Ordering::SeqCst);
                ready.context("Helper did not write its startup marker in time")?;
            }
            anyhow::Ok(())
        };
        let progress: Progress = Arc::new(|_, _| {});
        let (result, stopped) = tokio::join!(
            process(
                &mut command,
                None,
                &control,
                &progress,
                if cancel {
                    Duration::from_secs(10)
                } else {
                    Duration::from_secs(3)
                }
            ),
            stop
        );
        stopped?;
        assert!(result.is_err());
        let child = fs::read_to_string(&pid)?;
        assert!(!Path::new("/proc").join(child).exists());
        fs::remove_file(&pid)?;
    }
    Ok(())
}

// @variants: both
#[tokio::test]
async fn retains_staging_and_location_lease_until_an_abandoned_helper_exits() -> Result<()> {
    let fixture = Fixture::new(
        "pathlib.Path('temporary/pid').write_text(str(os.getpid()))\nprint(json.dumps(dict(protocol=1,type='progress',completed=1,total=1)),flush=True)\ntime.sleep(60)",
    )?;
    let (ready, receive) = tokio::sync::oneshot::channel();
    let ready = std::sync::Mutex::new(Some(ready));
    let task = tokio::spawn(transform(
        fixture.backend.clone(),
        fixture.inputs.clone(),
        fixture.parent.clone(),
        fixture.job.clone(),
        Arc::new(AtomicBool::new(false)),
        Arc::new(move |_, _| {
            if let Some(sender) = ready.lock().unwrap().take() {
                let _ = sender.send(());
            }
        }),
    ));
    tokio::time::timeout(Duration::from_secs(5), receive).await??;
    assert!(activity_lock().try_write_owned().is_err());
    let stage = fs::read_dir(&fixture.parent)?
        .next()
        .context("Missing stage")??
        .path();
    let pid = fs::read_to_string(stage.join("temporary/pid"))?;
    task.abort();
    assert!(task.await.is_err());
    tokio::time::timeout(Duration::from_secs(5), async {
        while stage.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert!(!Path::new("/proc").join(pid).exists());
    fixture.empty()
}

// @variants: both
#[tokio::test]
async fn rejects_noisy_helpers_without_filling_pipe_buffers() -> Result<()> {
    for stream in ["sys.stdout", "sys.stderr"] {
        let fixture = Fixture::new(&format!(
            "{stream}.write('x' * 2000000)\n{stream}.flush()\ntime.sleep(60)"
        ))?;
        let result = tokio::time::timeout(Duration::from_secs(5), fixture.run()).await?;
        assert!(result.is_err());
        fixture.empty()?;
    }
    Ok(())
}

#[test]
fn rejects_protocol_drift_duplicate_keys_and_misordered_progress() -> Result<()> {
    for patch in [
        json!({"protocol":2}),
        json!({"version":"0.7.1"}),
        json!({"version":"0.8.0"}),
        json!({"version":"0.9.0"}),
        json!({"version":"0.9.1"}),
        json!({"version":"99.0.0"}),
        json!({"backend":"other"}),
        json!({"games":["LE2"]}),
        json!({"capabilities":[]}),
        json!({"capabilities":["le1-tlk","le1-tlk"]}),
    ] {
        let mut value = capabilities();
        value
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(
            serde_json::from_value::<Capabilities>(value)?
                .validate("le1-tlk", super::super::Target::Le1)
                .is_err()
        );
    }
    for bytes in [
        r#"{"protocol":1,"protocol":1,"type":"progress","completed":1,"total":1}"#,
        r#"{"protocol":1,"type":"progress","completed":0,"total":1}"#,
        r#"{"protocol":1,"type":"progress","completed":2,"total":2}"#,
        r#"{"protocol":1,"type":"progress","completed":1,"total":1,"unknown":true}"#,
        r#"{"protocol":1,"type":"error","code":"failure"}"#,
    ] {
        assert!(
            Transcript::default().accept(bytes.as_bytes()).is_err(),
            "{bytes}"
        );
    }
    let mut transcript = Transcript::default();
    transcript.accept(br#"{"protocol":1,"type":"progress","completed":1,"total":2}"#)?;
    assert!(
        transcript
            .accept(br#"{"protocol":1,"type":"progress","completed":2,"total":3}"#)
            .is_err()
    );
    assert!(transcript.finish().is_err());
    Ok(())
}

// @variants: both
#[test]
fn rejects_links_case_collisions_and_foreign_engine_anchors() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let control = control();
    for path in [
        "../Engine.pcc",
        "../system/file",
        "~docs~/file",
        "CookedPCConsole/../file.pcc",
        "CookedPCConsole/NPCs/../../file.pcc",
        "DLC/DLC_MOD_Test/CookedPCConsole/NPCs/../file.pcc",
        "DLC/DLC_MOD_Test/CookedPCConsole/NPCs./file.pcc",
        "CookedPCConsole\\file.pcc",
        "CookedPCConsole/file.pcc.",
        "/CookedPCConsole/file.pcc",
    ] {
        assert!(!tlk_target(path), "{path}");
    }
    let file = fixture.inputs.candidate.join("CookedPCConsole/Example.pcc");
    fs::hard_link(&file, fixture.root.path().join("linked"))?;
    assert!(
        prepare(
            &fixture.backend,
            &fixture.inputs,
            &fixture.parent,
            &fixture.job,
            &control
        )
        .is_err()
    );
    fs::remove_file(fixture.root.path().join("linked"))?;
    let original = fixture.root.path().join("source");
    fs::rename(&file, &original)?;
    symlink(&original, &file)?;
    assert!(
        prepare(
            &fixture.backend,
            &fixture.inputs,
            &fixture.parent,
            &fixture.job,
            &control
        )
        .is_err()
    );
    fs::remove_file(&file)?;
    fs::rename(original, &file)?;
    let stage = prepare(
        &fixture.backend,
        &fixture.inputs,
        &fixture.parent,
        &fixture.job,
        &control,
    )?;
    let output = stage.path().join("output");
    fs::create_dir(output.join("CookedPCConsole"))?;
    symlink(&file, output.join("CookedPCConsole/Example.pcc"))?;
    let target = identity(&fixture.inputs.candidate, "CookedPCConsole/Example.pcc")?;
    assert!(
        files::outputs(
            &output,
            std::slice::from_ref(&target.path),
            std::slice::from_ref(&target),
            2 * 1024 * 1024 * 1024,
            &control
        )
        .is_err()
    );
    let mut second = target.clone();
    second.path.make_ascii_lowercase();
    assert!(
        files::outputs(
            &output,
            &[target.path.clone(), second.path.clone()],
            &[target, second],
            2 * 1024 * 1024 * 1024,
            &control
        )
        .is_err()
    );
    Ok(())
}

// @variants: snap
#[test]
fn uses_explicit_granted_roots_and_rejects_unavailable_access() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let mut inputs = fixture.inputs.clone();
    inputs.game = fixture.root.path().join("revoked-grant");
    assert!(
        prepare(
            &fixture.backend,
            &inputs,
            &fixture.parent,
            &fixture.job,
            &control()
        )
        .is_err()
    );
    let mut inputs = fixture.inputs.clone();
    inputs.candidate = inputs.game.clone();
    assert!(
        prepare(
            &fixture.backend,
            &inputs,
            &fixture.parent,
            &fixture.job,
            &control()
        )
        .is_err()
    );
    let command = fixture.backend.command(&fixture.parent)?;
    assert_eq!(command.as_std().get_program(), fixture.backend.runtime);
    let environment: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
    assert_eq!(
        environment.get(std::ffi::OsStr::new("HOME")),
        Some(&Some(fixture.parent.join("temporary").as_os_str()))
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn rejects_cancellation_and_missing_helpers_before_staging() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let result = transform(
        fixture.backend.clone(),
        fixture.inputs.clone(),
        fixture.parent.clone(),
        fixture.job.clone(),
        Arc::new(AtomicBool::new(true)),
        Arc::new(|_, _| {}),
    )
    .await;
    assert!(result.is_err());
    fixture.empty()?;
    fs::remove_file(&fixture.backend.native_library)?;
    assert!(fixture.run().await.is_err());
    fixture.empty()
}

// @variants: both
#[tokio::test]
async fn cancellation_while_waiting_for_folder_access_never_starts_a_helper() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let lock = activity_lock().write_owned().await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(transform(
        fixture.backend.clone(),
        fixture.inputs.clone(),
        fixture.parent.clone(),
        fixture.job.clone(),
        cancelled.clone(),
        Arc::new(|_, _| {}),
    ));
    tokio::task::yield_now().await;
    cancelled.store(true, Ordering::SeqCst);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await??
            .is_err()
    );
    fixture.empty()?;
    drop(lock);
    Ok(())
}

// @variants: both
#[tokio::test]
async fn discards_outputs_if_candidate_inputs_change_during_helper_work() -> Result<()> {
    let fixture = Fixture::new(&format!(
        "{SUCCESS}\n(pathlib.Path(request['input_root']) / request['targets'][0]['path']).write_bytes(b'changed!')"
    ))?;
    assert!(fixture.run().await.is_err());
    assert_eq!(fs::read_dir(&fixture.parent)?.count(), 0);
    assert_eq!(
        fs::read(fixture.inputs.candidate.join("CookedPCConsole/Example.pcc"))?,
        b"changed!"
    );
    Ok(())
}

// @variants: both
#[tokio::test]
async fn terminates_helper_if_progress_delivery_panics() -> Result<()> {
    let fixture = Fixture::new(
        "print(json.dumps(dict(protocol=1,type='progress',completed=1,total=1)),flush=True)\ntime.sleep(60)",
    )?;
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        transform(
            fixture.backend.clone(),
            fixture.inputs.clone(),
            fixture.parent.clone(),
            fixture.job.clone(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|_, _| panic!("progress receiver failed")),
        ),
    )
    .await?;
    assert!(result.is_err());
    fixture.empty()
}

// @variants: both
#[test]
fn refuses_changed_inputs_and_backends_inside_package_content() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let mut backend = fixture.backend.clone();
    backend.runtime = fixture.inputs.candidate.join("installer");
    fs::copy(&fixture.backend.runtime, &backend.runtime)?;
    assert!(
        prepare(
            &backend,
            &fixture.inputs,
            &fixture.parent,
            &fixture.job,
            &control()
        )
        .is_err()
    );
    fs::write(
        fixture.inputs.candidate.join("CookedPCConsole/Example.pcc"),
        b"modified",
    )?;
    assert!(
        prepare(
            &fixture.backend,
            &fixture.inputs,
            &fixture.parent,
            &fixture.job,
            &control()
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(&fixture.parent)?.count(), 0);
    Ok(())
}

#[test]
fn rejects_completion_without_the_requested_work() -> Result<()> {
    let fixture = Fixture::new(SUCCESS)?;
    let output = json!({"protocol":1,"type":"complete","outputs":[{"path":"CookedPCConsole/Example.pcc","size":1,"sha256":"0".repeat(64)}]});
    let mut transcript = Transcript::for_job(&fixture.job);
    assert!(transcript.accept(&serde_json::to_vec(&output)?).is_err());
    assert!(
        transcript
            .accept(br#"{"protocol":1,"type":"progress","completed":1,"total":2}"#)
            .is_err()
    );
    Ok(())
}

// @variants: both
#[test]
fn accepts_tlk_packages_in_nested_cooked_folders() {
    for path in [
        "CookedPCConsole/NPCs/Other NPCs/Example.pcc",
        "DLC/DLC_MOD_Test/CookedPCConsole/NPCs/Other NPCs/Example.pcc",
    ] {
        assert!(tlk_target(path), "{path}");
    }
}

// @variants: both
#[tokio::test]
async fn surfaces_runtime_startup_errors_and_discards_outputs() -> Result<()> {
    let fixture = Fixture::new("print('ICU could not be loaded', file=sys.stderr)\nsys.exit(134)")?;
    let error = fixture
        .run()
        .await
        .err()
        .context("Expected startup failure")?;
    let message = error.to_string();
    assert!(message.contains("ICU could not be loaded"), "{message}");
    assert!(message.contains("134"), "{message}");
    assert!(!message.contains("expected value"));
    fixture.empty()
}

// @variants: both
#[test]
fn bounds_runtime_diagnostics_and_reports_signal_exits() {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(6);
    let message = startup_failure(status, b"");
    assert!(message.contains("signal"));
    assert!(message.contains("without reporting a reason"));
    let mut bytes = vec![b'x'; 5000];
    bytes[0] = 0x1b;
    bytes[1] = 0xff;
    let message = startup_failure(status, &bytes);
    assert!(message.len() < 2200);
    assert!(!message.contains('\u{1b}'));
}

// @variants: both
#[test]
fn confines_content_library_search_to_the_running_snap() -> Result<()> {
    let package = tempfile::tempdir()?;
    let backend = Backend {
        runtime: package.path().join("usr/lib/deployd/mele/runtime/dotnet"),
        assembly: package
            .path()
            .join("usr/lib/deployd/mele/app/Deployd.Mele.dll"),
        native_library: package
            .path()
            .join("usr/lib/deployd/mele/app/libdeployd_oodle.so"),
    };
    let stage = tempfile::tempdir()?;
    let other = tempfile::tempdir()?;
    for snap in [Some(package.path()), None, Some(other.path())] {
        let command = backend.command_with_snap(stage.path(), snap)?;
        let environment: std::collections::BTreeMap<_, _> = command.as_std().get_envs().collect();
        let paths = environment
            .get(std::ffi::OsStr::new("LD_LIBRARY_PATH"))
            .copied()
            .flatten()
            .context("Missing library search path")?;
        let paths: Vec<_> = std::env::split_paths(paths).collect();
        assert_eq!(
            paths.contains(&package.path().join("gpu-2404/usr/lib/x86_64-linux-gnu")),
            snap == Some(package.path())
        );
        assert!(paths.iter().all(|path| path.starts_with(package.path())));
        assert!(!paths.iter().any(|path| path.starts_with(other.path())));
    }
    Ok(())
}
