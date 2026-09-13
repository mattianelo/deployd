use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::Command;

use super::operation::{Control, Lease};

use protocol::{Capabilities, HelperError, Transcript};
pub(crate) use protocol::{FileIdentity, Job};

mod dlc_jobs;
mod files;
pub(super) mod jobs;
pub(super) mod m3m;
pub(super) mod protocol;

#[derive(Debug, Clone)]
pub(crate) struct Backend {
    pub(crate) runtime: PathBuf,
    pub(crate) assembly: PathBuf,
    pub(crate) native_library: PathBuf,
}

#[derive(Clone)]
pub(crate) struct Inputs {
    pub(crate) game: PathBuf,
    pub(crate) candidate: PathBuf,
    pub(crate) original: Option<PathBuf>,
}

pub(crate) struct ValidatedOutput {
    stage: TempDir,
    pub(crate) files: Vec<FileIdentity>,
}

impl ValidatedOutput {
    pub(crate) fn root(&self) -> PathBuf {
        self.stage.path().join("output")
    }
}

struct Abandon(Arc<AtomicBool>);

impl Drop for Abandon {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

type Progress = Arc<dyn Fn(u32, u32) + Send + Sync>;

pub(crate) async fn transform(
    backend: Backend,
    inputs: Inputs,
    staging_parent: PathBuf,
    job: Job,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<ValidatedOutput> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandon = Abandon(abandoned.clone());
    let control = Control {
        cancelled,
        abandoned,
    };
    // The worker retains staging and folder leases until the child is reaped, even
    // if its caller drops the future. Only disposable, reconstructible files are written.
    let worker_control = control.clone();
    let result = tokio::spawn(async move {
        let control = worker_control;
        let lease = Lease::acquire(&control).await?;
        transform_with_lease(
            backend,
            inputs,
            staging_parent,
            job,
            control,
            progress,
            lease,
        )
        .await
    })
    .await
    .context("MELE transformation worker failed")?;
    control.check()?;
    result
}

pub(super) async fn transform_with_lease(
    backend: Backend,
    inputs: Inputs,
    staging_parent: PathBuf,
    job: Job,
    control: Control,
    progress: Progress,
    _lease: Arc<Lease>,
) -> Result<ValidatedOutput> {
    control.check()?;
    let prepared = {
        let backend = backend.clone();
        let inputs = inputs.clone();
        let job = job.clone();
        let control = control.clone();
        tokio::task::spawn_blocking(move || {
            prepare(&backend, &inputs, &staging_parent, &job, &control)
        })
        .await
        .context("MELE staging preparation failed")??
    };
    let mut command = backend.command(prepared.path())?;
    command.arg("capabilities");
    let capabilities = process(
        &mut command,
        None,
        &control,
        &progress,
        Duration::from_secs(10),
    )
    .await?;
    serde_json::from_slice::<Capabilities>(&capabilities)?.validate(job.operation(), job.game())?;

    let mut command = backend.command(prepared.path())?;
    command
        .arg(job.command())
        .arg(prepared.path().join("request.json"));
    let manifest = process(
        &mut command,
        Some(Transcript::for_job(&job)),
        &control,
        &progress,
        Duration::from_secs(30 * 60),
    )
    .await?;
    let outputs: Vec<FileIdentity> = serde_json::from_slice(&manifest)?;
    let expected = job.outputs();
    tokio::task::spawn_blocking(move || {
        control.check()?;
        files::outputs(
            &prepared.path().join("output"),
            &expected,
            &outputs,
            job.byte_limit(),
            &control,
        )?;
        verify_inputs(&inputs, &job, &control)?;
        control.check()?;
        Ok(ValidatedOutput {
            stage: prepared,
            files: outputs,
        })
    })
    .await
    .context("MELE output verification failed")?
}

impl Backend {
    fn validate(&self) -> Result<()> {
        for path in [&self.runtime, &self.assembly, &self.native_library] {
            files::directory(
                path.parent()
                    .context("Missing helper installation folder")?,
            )?;
            let metadata = fs::symlink_metadata(path)
                .context("The packaged MELE helper is unavailable; repair the Deployd package")?;
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "The packaged MELE helper contains an invalid file"
            );
        }
        ensure!(
            self.native_library.parent() == self.assembly.parent(),
            "The MELE native library must accompany its managed helper"
        );
        ensure!(
            self.assembly
                .file_name()
                .is_some_and(|name| name == "Deployd.Mele.dll")
                && self
                    .native_library
                    .file_name()
                    .is_some_and(|name| name == "libdeployd_oodle.so"),
            "Unexpected MELE backend files"
        );
        Ok(())
    }

    fn command(&self, stage: &Path) -> Result<Command> {
        self.command_with_snap(stage, std::env::var_os("SNAP").as_deref().map(Path::new))
    }

    fn command_with_snap(&self, stage: &Path, snap: Option<&Path>) -> Result<Command> {
        let native = self
            .native_library
            .parent()
            .context("Missing MELE native library folder")?;
        let runtime = self
            .runtime
            .parent()
            .context("Missing MELE runtime folder")?;
        let mut libraries = vec![native.to_path_buf()];
        if let Some(prefix) = runtime.ancestors().nth(4)
            && runtime == prefix.join("lib/deployd/mele/runtime")
        {
            libraries.push(prefix.join("lib"));
            libraries.push(prefix.join("lib/x86_64-linux-gnu"));
        }
        if let Some(snap) = snap
            && runtime == snap.join("usr/lib/deployd/mele/runtime")
        {
            // Snapcraft omits staged libraries supplied by the connected GPU content snap.
            libraries.push(snap.join("gpu-2404/usr/lib/x86_64-linux-gnu"));
        }
        let mut command = Command::new(&self.runtime);
        command
            .arg(&self.assembly)
            .current_dir(stage)
            .env_clear()
            .env("DOTNET_ROOT", runtime)
            .env("DOTNET_EnableDiagnostics", "0")
            .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
            .env("DOTNET_NOLOGO", "1")
            .env("DOTNET_BUNDLE_EXTRACT_BASE_DIR", stage.join("temporary"))
            .env("LD_LIBRARY_PATH", std::env::join_paths(libraries)?)
            .env("HOME", stage.join("temporary"))
            .env("TMPDIR", stage.join("temporary"))
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        Ok(command)
    }
}

fn overlaps(first: &Path, second: &Path) -> bool {
    first.starts_with(second) || second.starts_with(first)
}

fn prepare(
    backend: &Backend,
    inputs: &Inputs,
    parent: &Path,
    job: &Job,
    control: &Control,
) -> Result<TempDir> {
    backend.validate()?;
    files::directory(parent)?;
    files::directory(&inputs.game)?;
    files::directory(&inputs.candidate)?;
    ensure!(
        !overlaps(&inputs.game, &inputs.candidate),
        "Helper candidates must be separate copies outside the game"
    );
    let roots = [&inputs.game, &inputs.candidate];
    for root in roots.into_iter().chain(inputs.original.as_ref()) {
        files::directory(root)?;
        for binary in [&backend.runtime, &backend.assembly, &backend.native_library] {
            ensure!(
                !binary.starts_with(root) && !binary.starts_with(parent),
                "The MELE backend cannot come from game, input, or staging folders"
            );
        }
        ensure!(
            !overlaps(root, parent),
            "Helper staging must be separate from game and input folders"
        );
    }
    job.validate()?;
    if !matches!(job, Job::Tlk { .. } | Job::SquadUi { .. }) {
        let original = inputs
            .original
            .as_ref()
            .context("This merge requires verified original inputs")?;
        ensure!(
            !overlaps(original, &inputs.candidate) && !overlaps(original, &inputs.game),
            "Originals must be separate from game and candidate folders"
        );
    } else {
        ensure!(
            inputs.original.is_none(),
            "This transformation does not accept an original root"
        );
    }
    verify_inputs(inputs, job, control)?;
    let stage = tempfile::Builder::new()
        .prefix("mele-transform-")
        .tempdir_in(parent)?;
    fs::create_dir(stage.path().join("output"))?;
    fs::create_dir(stage.path().join("temporary"))?;
    #[derive(Serialize)]
    struct Request<'a> {
        protocol: u32,
        game_root: &'a Path,
        input_root: &'a Path,
        #[serde(skip_serializing_if = "Option::is_none")]
        original_root: Option<&'a Path>,
        output_root: PathBuf,
        #[serde(flatten)]
        job: &'a Job,
    }
    let bytes = serde_json::to_vec(&Request {
        protocol: 1,
        game_root: &inputs.game,
        input_root: &inputs.candidate,
        original_root: inputs.original.as_deref(),
        output_root: stage.path().join("output"),
        job,
    })?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "MELE request exceeds its size limit"
    );
    fs::write(stage.path().join("request.json"), bytes)?;
    Ok(stage)
}

fn tlk_target(path: &str) -> bool {
    if files::relative(path).is_err() {
        return false;
    }
    let parts: Vec<_> = path.split('/').collect();
    let identifier = |text: &str| {
        !text.is_empty()
            && text.len() <= 255
            && text
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'_')
    };
    ((parts.len() >= 2 && parts[0] == "CookedPCConsole")
        || (parts.len() >= 4
            && parts[0] == "DLC"
            && identifier(parts[1])
            && parts[2] == "CookedPCConsole"))
        && parts
            .last()
            .and_then(|name| name.strip_suffix(".pcc"))
            .is_some_and(identifier)
}

fn verify_inputs(inputs: &Inputs, job: &Job, control: &Control) -> Result<()> {
    let mut total = 0_u64;
    let mut check = |root: &Path, file: &FileIdentity| -> Result<()> {
        total = total
            .checked_add(file.size)
            .context("MELE input size overflow")?;
        ensure!(
            total <= job.byte_limit(),
            "MELE inputs exceed the job size limit"
        );
        files::identity(root, file, control)
    };
    for input in job.inputs() {
        check(&inputs.candidate, input)?;
    }
    for original in job.originals() {
        check(
            inputs
                .original
                .as_deref()
                .context("Missing original inputs")?,
            original,
        )?;
    }
    Ok(())
}

async fn bounded<R: AsyncRead + Unpin>(reader: R, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    ensure!(
        bytes.len() <= limit,
        "MELE helper response exceeded its size limit"
    );
    Ok(bytes)
}

async fn messages<R: AsyncRead + Unpin>(
    reader: R,
    mut transcript: Transcript,
    progress: &Progress,
) -> Result<Transcript> {
    let mut reader = BufReader::new(reader);
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        loop {
            let chunk = reader.fill_buf().await?;
            if chunk.is_empty() {
                break;
            }
            let length = chunk
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(chunk.len(), |index| index + 1);
            total += length;
            ensure!(
                line.len() + length <= 1024 * 1024 && total <= 4 * 1024 * 1024,
                "MELE helper response exceeded its size limit"
            );
            line.extend_from_slice(&chunk[..length]);
            reader.consume(length);
            if line.last() == Some(&b'\n') {
                break;
            }
        }
        if line.is_empty() {
            break;
        }
        ensure!(
            line.last() == Some(&b'\n'),
            "Truncated helper protocol message"
        );
        if let Some((completed, total)) = transcript.accept(&line)? {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| progress(completed, total)))
                .map_err(|_| anyhow::anyhow!("MELE progress delivery failed"))?;
        }
    }
    Ok(transcript)
}

async fn process(
    command: &mut Command,
    transcript: Option<Transcript>,
    control: &Control,
    progress: &Progress,
    timeout: Duration,
) -> Result<Vec<u8>> {
    control.check()?;
    let mut child = command.spawn().context(
        "Cannot start the packaged MELE helper; check package integrity and folder access",
    )?;
    let stdout = child
        .stdout
        .take()
        .context("Helper stdout is unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Helper stderr is unavailable")?;
    let result = {
        let output = async {
            match transcript {
                Some(transcript) => Ok((None, Some(messages(stdout, transcript, progress).await?))),
                None => Ok((Some(bounded(stdout, 65536).await?), None)),
            }
        };
        let work = async {
            let (stdout, stderr, status) =
                tokio::try_join!(output, bounded(stderr, 65536), async {
                    child
                        .wait()
                        .await
                        .context("Cannot wait for the MELE helper")
                })?;
            if !status.success() {
                match serde_json::from_slice::<HelperError>(&stderr) {
                    Ok(error) => bail!("{}", error.message()?),
                    Err(_) => bail!("{}", startup_failure(status, &stderr)),
                }
            }
            ensure!(
                stderr.is_empty(),
                "MELE helper reported errors despite a successful exit"
            );
            match stdout {
                (Some(bytes), None) => Ok(bytes),
                (None, Some(transcript)) => Ok(serde_json::to_vec(&transcript.finish()?)?),
                _ => bail!("Invalid helper response state"),
            }
        };
        tokio::select! {
            biased;
            _ = control.stopped() => Err(anyhow::anyhow!("MELE transformation cancelled; staged outputs were discarded")),
            _ = tokio::time::sleep(timeout) => Err(anyhow::anyhow!("MELE helper timed out; staged outputs were discarded")),
            result = work => result,
        }
    };
    if result.is_err() {
        let _ = child.start_kill();
        child
            .wait()
            .await
            .context("Cannot reap the failed MELE helper")?;
    }
    control.check()?;
    result
}

fn startup_failure(status: std::process::ExitStatus, stderr: &[u8]) -> String {
    let detail: String = String::from_utf8_lossy(stderr)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(2048)
        .collect();
    let detail = detail.trim();
    if detail.is_empty() {
        format!(
            "MELE helper stopped ({status}) without reporting a reason. Check the package runtime and confinement logs."
        )
    } else {
        format!("MELE helper stopped ({status}):\n{detail}")
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod corpus;

impl Backend {
    pub(crate) fn packaged() -> Result<Self> {
        let executable =
            std::env::current_exe().context("Cannot locate the packaged MELE helper")?;
        Self::at(&Self::package_root(
            &executable,
            std::env::var_os("SNAP").as_deref(),
        )?)
    }

    fn package_root(
        executable: &std::path::Path,
        snap: Option<&std::ffi::OsStr>,
    ) -> Result<PathBuf> {
        if let Some(snap) = snap {
            let root = PathBuf::from(snap);
            anyhow::ensure!(
                root.is_absolute() && executable.is_absolute(),
                "MELE helper must belong to the running Snap"
            );
            let packaged_executable = fs::symlink_metadata(root.join("bin/deployd"))
                .context("Cannot verify Deployd's executable in the running Snap")?;
            let running_executable = fs::symlink_metadata(executable)
                .context("Cannot verify the running Deployd executable")?;
            // Parallel Snap mount aliases need file identity rather than textual containment.
            anyhow::ensure!(
                packaged_executable.is_file()
                    && running_executable.is_file()
                    && packaged_executable.dev() == running_executable.dev()
                    && packaged_executable.ino() == running_executable.ino(),
                "MELE helper must belong to the running Snap"
            );
            return Ok(root.join("usr/lib/deployd/mele"));
        }
        let bin = executable
            .parent()
            .context("Cannot locate Deployd's executable folder")?;
        Ok(bin
            .parent()
            .context("Cannot locate Deployd's installation")?
            .join("lib/deployd/mele"))
    }

    fn at(root: &std::path::Path) -> Result<Self> {
        let backend = Self {
            runtime: root.join("runtime/dotnet"),
            assembly: root.join("app/Deployd.Mele.dll"),
            native_library: root.join("app/libdeployd_oodle.so"),
        };
        for path in [&backend.runtime, &backend.assembly, &backend.native_library] {
            let metadata = std::fs::symlink_metadata(path).context(
                "The packaged MELE helper is missing; rebuild or reinstall this Deployd package",
            )?;
            anyhow::ensure!(
                metadata.is_file(),
                "The packaged MELE helper contains an unexpected file or link"
            );
        }
        Ok(backend)
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    // @variants: both
    #[test]
    fn helper_discovery_stays_inside_the_running_package() -> Result<()> {
        assert_eq!(
            Backend::package_root(std::path::Path::new("/app/usr/bin/deployd"), None)?,
            PathBuf::from("/app/usr/lib/deployd/mele")
        );
        let snap = tempfile::tempdir()?;
        fs::create_dir(snap.path().join("bin"))?;
        let executable = snap.path().join("bin/deployd");
        fs::write(&executable, b"executable fixture")?;
        assert_eq!(
            Backend::package_root(&executable, Some(snap.path().as_os_str()))?,
            snap.path().join("usr/lib/deployd/mele")
        );
        assert!(
            Backend::package_root(&executable, Some(std::ffi::OsStr::new("relative"))).is_err()
        );
        let missing = tempfile::tempdir()?;
        assert!(Backend::at(missing.path()).is_err());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn accepts_runtime_mount_aliases_but_rejects_different_executables() -> Result<()> {
        let package = tempfile::tempdir()?;
        let alias = tempfile::tempdir()?;
        fs::create_dir(package.path().join("bin"))?;
        let expected = package.path().join("bin/deployd");
        let actual = alias.path().join("deployd");
        fs::write(&expected, b"executable fixture")?;
        // These fixture links reproduce the device/inode identity of a bind mount.
        fs::hard_link(&expected, &actual)?;
        assert_eq!(
            Backend::package_root(&actual, Some(package.path().as_os_str()))?,
            package.path().join("usr/lib/deployd/mele")
        );
        fs::remove_file(&actual)?;
        fs::write(&actual, b"executable fixture")?;
        assert!(Backend::package_root(&actual, Some(package.path().as_os_str())).is_err());
        fs::remove_file(&actual)?;
        std::os::unix::fs::symlink(&expected, &actual)?;
        assert!(Backend::package_root(&actual, Some(package.path().as_os_str())).is_err());
        fs::remove_file(&actual)?;
        assert!(Backend::package_root(&actual, Some(package.path().as_os_str())).is_err());
        fs::create_dir(&actual)?;
        assert!(Backend::package_root(&actual, Some(package.path().as_os_str())).is_err());
        Ok(())
    }
}
