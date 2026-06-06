// declared_role: orchestration, validator, predicate, mapper, accessor, formatter

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[cfg(unix)]
use std::os::unix::process::{CommandExt, ExitStatusExt};

#[cfg(unix)]
extern "C" {
    fn setpgid(pid: std::os::raw::c_int, pgid: std::os::raw::c_int) -> std::os::raw::c_int;
    fn kill(pid: std::os::raw::c_int, sig: std::os::raw::c_int) -> std::os::raw::c_int;
}

const SIGTERM: std::os::raw::c_int = 15;
const SIGKILL: std::os::raw::c_int = 9;

pub struct RunningChild {
    child: Child,
    pid: u32,
}

pub struct ChildPipes {
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

impl RunningChild {
    pub fn spawn(
        argv: &[String],
        cwd: &str,
        env: &BTreeMap<String, String>,
        stdin_bytes: Vec<u8>,
    ) -> io::Result<(Self, ChildPipes)> {
        let mut command = child_command(argv, cwd, env);
        prepare_child_process_group(&mut command);
        let mut child = spawn_command(&mut command)?;
        let pid = child.id();
        let pipes = child_pipes(&mut child)?;
        launch_stdin_writer(take_stdin_pipe(&mut child), stdin_bytes);

        Ok((running_child(child, pid), pipes))
    }

    pub fn wait_with_deadline(&mut self, deadline_unix_ms: Option<u64>) -> Value {
        loop {
            if let Some(status) = self.poll_status() {
                return status;
            }

            if deadline_elapsed(deadline_unix_ms) {
                return self.cancel_for_deadline();
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn poll_status(&mut self) -> Option<Value> {
        wait_poll_status(&mut self.child)
    }

    pub fn cancel_for_deadline(&mut self) -> Value {
        self.terminate_group();
        cancelled_status()
    }

    pub fn terminate_descendants(&self) {
        self.signal_group(SIGTERM);
        thread::sleep(Duration::from_millis(50));
        self.signal_group(SIGKILL);
    }

    fn terminate_group(&mut self) {
        self.signal_group(SIGTERM);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            if child_exited(&mut self.child) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.signal_group(SIGKILL);
        let _ = self.child.wait();
    }

    fn signal_group(&self, signal: std::os::raw::c_int) {
        #[cfg(unix)]
        unsafe {
            let _ = kill(-(self.pid as std::os::raw::c_int), signal);
        }

        #[cfg(not(unix))]
        let _ = signal;
    }
}

fn child_command(argv: &[String], cwd: &str, env: &BTreeMap<String, String>) -> Command {
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        // hermetic: declared env only, inherited env cleared
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    command
}

fn prepare_child_process_group(command: &mut Command) {
    #[cfg(unix)]
    install_child_process_group_pre_exec(command);

    #[cfg(not(unix))]
    let _ = command;
}

#[cfg(unix)]
fn install_child_process_group_pre_exec(command: &mut Command) {
    unsafe {
        command.pre_exec(child_process_group_setup);
    }
}

#[cfg(unix)]
fn child_process_group_setup() -> io::Result<()> {
    process_group_setup_result(process_group_setup_succeeded())
}

#[cfg(unix)]
fn process_group_setup_succeeded() -> bool {
    unsafe { setpgid(0, 0) == 0 }
}

#[cfg(unix)]
fn process_group_setup_result(succeeded: bool) -> io::Result<()> {
    if succeeded {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn spawn_command(command: &mut Command) -> io::Result<Child> {
    command.spawn()
}

fn child_pipes(child: &mut Child) -> io::Result<ChildPipes> {
    let (stdout, stderr) = take_child_pipes(child);
    let stdout = require_stdout_pipe(stdout)?;
    let stderr = require_stderr_pipe(stderr)?;
    Ok(child_pipes_value(stdout, stderr))
}

fn take_child_pipes(child: &mut Child) -> (Option<ChildStdout>, Option<ChildStderr>) {
    (child.stdout.take(), child.stderr.take())
}

fn require_stdout_pipe(stdout: Option<ChildStdout>) -> io::Result<ChildStdout> {
    stdout.ok_or_else(|| io::Error::other("child stdout pipe unavailable"))
}

fn require_stderr_pipe(stderr: Option<ChildStderr>) -> io::Result<ChildStderr> {
    stderr.ok_or_else(|| io::Error::other("child stderr pipe unavailable"))
}

fn child_pipes_value(stdout: ChildStdout, stderr: ChildStderr) -> ChildPipes {
    ChildPipes { stdout, stderr }
}

fn take_stdin_pipe(child: &mut Child) -> Option<ChildStdin> {
    child.stdin.take()
}

fn launch_stdin_writer(stdin: Option<ChildStdin>, stdin_bytes: Vec<u8>) {
    if let Some(mut stdin) = stdin {
        thread::spawn(move || {
            let _ = stdin.write_all(&stdin_bytes);
        });
    }
}

fn running_child(child: Child, pid: u32) -> RunningChild {
    RunningChild { child, pid }
}

fn wait_poll_status(child: &mut Child) -> Option<Value> {
    wait_poll_value(child.try_wait())
}

fn child_exited(child: &mut Child) -> bool {
    child.try_wait().ok().flatten().is_some()
}

fn wait_poll_value(result: io::Result<Option<ExitStatus>>) -> Option<Value> {
    match result {
        Ok(Some(status)) => Some(status_value(status)),
        Ok(None) => None,
        Err(error) => Some(wait_failed_status(&error)),
    }
}

fn deadline_elapsed(deadline_unix_ms: Option<u64>) -> bool {
    deadline_unix_ms.is_some_and(|deadline| crate::encoding::now_unix_ms() >= deadline)
}

enum ChildStatusKind {
    Exited(i32),
    SignalTerminated(i32),
    Unknown,
}

fn status_value(status: ExitStatus) -> Value {
    status_kind_value(status_kind(status))
}

fn status_kind(status: ExitStatus) -> ChildStatusKind {
    if let Some(code) = status.code() {
        return ChildStatusKind::Exited(code);
    }

    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return ChildStatusKind::SignalTerminated(signal);
    }

    ChildStatusKind::Unknown
}

fn status_kind_value(kind: ChildStatusKind) -> Value {
    match kind {
        ChildStatusKind::Exited(code) => json!({ "kind": "exited", "code": code }),
        ChildStatusKind::SignalTerminated(signal) => {
            json!({ "kind": "signal_terminated", "signal": signal })
        }
        ChildStatusKind::Unknown => json!({ "kind": "unknown" }),
    }
}

fn wait_failed_status(error: &io::Error) -> Value {
    json!({ "kind": "spawn_error", "reason": format!("wait failed: {error}") })
}

fn cancelled_status() -> Value {
    json!({ "kind": "cancelled" })
}
