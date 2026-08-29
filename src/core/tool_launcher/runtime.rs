use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::utils::paths;

use super::launch_plan::LaunchPlan;

const TOOL_TERMINATE_GRACE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone)]
pub struct ToolProcessHandle {
    pub pid: u32,
    process_group_id: Option<i32>,
    cancel: Arc<AtomicBool>,
}

pub struct ToolLaunchHooks {
    pub cancel: Arc<AtomicBool>,
    pub on_spawn: Option<Box<dyn FnOnce(ToolProcessHandle) + Send + 'static>>,
    pub on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
}

impl ToolProcessHandle {
    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let handle = self.clone();
        std::thread::spawn(move || handle.terminate_process_tree());
    }

    fn terminate_process_tree(&self) {
        terminate_target_with(
            self.process_target(),
            signals::send,
            signals::exists,
            std::thread::sleep,
        );
    }

    fn process_target(&self) -> ProcessTarget {
        #[cfg(unix)]
        {
            self.process_group_id
                .map(ProcessTarget::Group)
                .unwrap_or(ProcessTarget::Process(self.pid))
        }
        #[cfg(not(unix))]
        {
            ProcessTarget::Process(self.pid)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessTarget {
    Process(u32),
    #[cfg(unix)]
    Group(i32),
}

fn terminate_target_with(
    target: ProcessTarget,
    send: impl Fn(ProcessTarget, libc::c_int),
    exists: impl Fn(ProcessTarget) -> bool,
    wait: impl Fn(Duration),
) {
    send(target, libc::SIGTERM);
    wait(TOOL_TERMINATE_GRACE);
    if exists(target) {
        send(target, libc::SIGKILL);
    }
}

pub(super) fn supervise(mut plan: LaunchPlan, hooks: ToolLaunchHooks) -> Result<u32> {
    log_tool_command(&plan.tool_name, &plan.command);
    plan.command.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        plan.command.process_group(0);
    }

    let child = spawn_process_with(&mut plan.command, &plan.tool_name, |command| {
        command.spawn()
    })?;
    let process_id = child.id();
    diagnostic_log(&format!(
        "deployd-tool-debug: spawned '{}' pid={process_id}",
        plan.tool_name
    ));
    let handle = ToolProcessHandle {
        pid: process_id,
        #[cfg(unix)]
        process_group_id: Some(process_id as i32),
        #[cfg(not(unix))]
        process_group_id: None,
        cancel: hooks.cancel,
    };
    if let Some(callback) = hooks.on_spawn {
        callback(handle);
    }

    supervise_exit(child, plan.tool_name, hooks.on_exit);

    Ok(process_id)
}

fn supervise_exit(
    mut child: std::process::Child,
    tool_name: String,
    on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
) {
    let stderr_thread = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buffer = String::new();
            let _ = std::io::BufReader::new(stderr).read_to_string(&mut buffer);
            buffer
        })
    });
    std::thread::spawn(move || {
        let wait_result = child.wait();
        let stderr = stderr_thread
            .and_then(|thread| thread.join().ok())
            .filter(|output| !output.is_empty());

        match &wait_result {
            Ok(status) => diagnostic_log(&format!(
                "deployd-tool-debug: '{tool_name}' wait status={status}"
            )),
            Err(error) => diagnostic_log(&format!(
                "deployd-tool-debug: '{tool_name}' wait failed: {error}"
            )),
        }
        if let Some(stderr) = &stderr {
            diagnostic_log(&format!(
                "deployd-tool-debug: '{tool_name}' stderr tail:\n{}",
                tail_for_log(stderr)
            ));
        }

        let error = match wait_result {
            Ok(status) if !status.success() => {
                if let Some(stderr) = &stderr {
                    eprintln!("deployd: {tool_name} exited {status}. stderr:\n{stderr}");
                } else {
                    eprintln!("deployd: {tool_name} exited {status} (no stderr).");
                }
                Some(stderr.unwrap_or_else(|| format!("process exited with {status}")))
            }
            Err(error) => {
                eprintln!("deployd: failed to wait on process: {error}");
                Some(error.to_string())
            }
            _ => None,
        };
        if let Some(callback) = on_exit {
            callback(error);
        }
    });
}

fn spawn_process_with<T>(
    command: &mut std::process::Command,
    tool_name: &str,
    spawn: impl FnOnce(&mut std::process::Command) -> std::io::Result<T>,
) -> Result<T> {
    spawn(command).map_err(|error| {
        anyhow!(
            "Could not start process for \"{}\".\nError: {error}",
            tool_name
        )
    })
}

fn log_tool_command(tool_name: &str, command: &std::process::Command) {
    diagnostic_log(&format!(
        "deployd-tool-debug: launching '{tool_name}' program={}",
        command.get_program().to_string_lossy()
    ));
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    diagnostic_log(&format!("deployd-tool-debug: args={arguments:?}"));
    diagnostic_log(&format!(
        "deployd-tool-debug: cwd={}",
        command
            .get_current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<inherit>".to_string())
    ));

    for key in [
        "APPDIR",
        "APPIMAGE",
        "GDK_PIXBUF_MODULEDIR",
        "GDK_PIXBUF_MODULE_FILE",
        "GIO_MODULE_DIR",
        "GI_TYPELIB_PATH",
        "GSETTINGS_SCHEMA_DIR",
        "GTK_PATH",
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "PROTONPATH",
        "STEAM_COMPAT_DATA_PATH",
        "UMU_FOLDERS_PATH",
        "WINEDLLOVERRIDES",
        "WINEDEBUG",
        "WINEPREFIX",
    ] {
        diagnostic_log(&format!(
            "deployd-tool-debug: env {key}={}",
            command_env_value(command, key)
        ));
    }
}

fn command_env_value(command: &std::process::Command, key: &str) -> String {
    if let Some((_, value)) = command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
    {
        return value
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<removed>".to_string());
    }
    std::env::var(key).unwrap_or_else(|_| "<unset>".to_string())
}

pub(super) fn diagnostic_log(message: &str) {
    eprintln!("{message}");
    let Ok(data_dir) = paths::deployd_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&data_dir);
    let log_path = data_dir.join("tool-launch-debug.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{message}");
    }
}

fn tail_for_log(text: &str) -> String {
    const MAX_CHARS: usize = 20_000;
    let length = text.chars().count();
    if length <= MAX_CHARS {
        return text.to_string();
    }
    let tail = text
        .chars()
        .skip(length.saturating_sub(MAX_CHARS))
        .collect::<String>();
    format!("<truncated to last {MAX_CHARS} chars>\n{tail}")
}

pub(super) fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

pub(super) fn ensure_not_cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    if is_cancelled(cancel) {
        Err(anyhow!("Tool launch cancelled"))
    } else {
        Ok(())
    }
}

pub(super) fn run_output_cancellable(
    command: &mut std::process::Command,
    cancel: Option<&AtomicBool>,
) -> Result<Output> {
    ensure_not_cancelled(cancel)?;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("start Wine setup command")?;
    loop {
        if is_cancelled(cancel) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("Tool launch cancelled"));
        }
        if child.try_wait()?.is_some() {
            return child
                .wait_with_output()
                .context("collect Wine setup command output");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

mod signals {
    use super::ProcessTarget;

    pub(super) fn send(target: ProcessTarget, signal: libc::c_int) {
        match target {
            ProcessTarget::Process(process_id) => {
                // SAFETY: the pid comes from `Child::id` and `signal` is a libc signal
                // constant. A stale pid only causes `kill` to return an ignored error.
                unsafe {
                    libc::kill(process_id as libc::pid_t, signal);
                }
            }
            #[cfg(unix)]
            ProcessTarget::Group(process_group_id) => {
                // SAFETY: the negative id targets the group created for this child by
                // `CommandExt::process_group(0)`, so Deployd never signals an inherited group.
                unsafe {
                    libc::kill(-process_group_id, signal);
                }
            }
        }
    }

    pub(super) fn exists(target: ProcessTarget) -> bool {
        match target {
            ProcessTarget::Process(process_id) => {
                // SAFETY: signal 0 only probes the pid returned by `Child::id`; it does
                // not deliver a signal or dereference memory.
                unsafe { libc::kill(process_id as libc::pid_t, 0) == 0 }
            }
            #[cfg(unix)]
            ProcessTarget::Group(process_group_id) => {
                // SAFETY: signal 0 only probes the Deployd-created process group and
                // does not deliver a signal.
                unsafe { libc::kill(-process_group_id, 0) == 0 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn spawn_failure_names_the_tool_and_os_error() {
        let mut command = Command::new("ignored-by-test-boundary");
        let result: Result<()> = spawn_process_with(&mut command, "Missing Tool", |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "confined"))
        });

        let error = result.err().expect("missing command must fail to spawn");
        assert!(error.to_string().contains("Missing Tool"));
        assert!(error.to_string().contains("Could not start process"));
        assert!(error.to_string().contains("confined"));
    }

    #[cfg(unix)]
    #[test]
    fn termination_escalates_when_process_group_survives_grace_period() {
        let sent = RefCell::new(Vec::new());
        let target = ProcessTarget::Group(42);

        terminate_target_with(
            target,
            |target, signal| sent.borrow_mut().push((target, signal)),
            |probed| probed == target,
            |_| {},
        );

        assert_eq!(
            sent.into_inner(),
            vec![(target, libc::SIGTERM), (target, libc::SIGKILL)]
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_stops_the_deployd_owned_process_group() -> Result<()> {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        let plan = LaunchPlan {
            command,
            tool_name: "Cancellation Test".to_string(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (spawn_sender, spawn_receiver) = mpsc::channel();
        let (exit_sender, exit_receiver) = mpsc::channel();

        supervise(
            plan,
            ToolLaunchHooks {
                cancel: cancel.clone(),
                on_spawn: Some(Box::new(move |handle| {
                    let _ = spawn_sender.send(handle);
                })),
                on_exit: Some(Box::new(move |error| {
                    let _ = exit_sender.send(error);
                })),
            },
        )?;
        let handle = spawn_receiver.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(handle.process_group_id, Some(handle.pid as i32));

        handle.request_stop();
        let exit_error = exit_receiver.recv_timeout(Duration::from_secs(5))?;

        assert!(cancel.load(Ordering::SeqCst));
        assert!(
            exit_error.is_some(),
            "signal termination must report a non-zero exit"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let target = ProcessTarget::Group(handle.pid as i32);
        while signals::exists(target) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!signals::exists(target));
        Ok(())
    }
}
