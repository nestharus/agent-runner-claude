// declared_role: orchestration, formatter

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
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

        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if setpgid(0, 0) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }

        let mut child = command.spawn()?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child stdout pipe unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("child stderr pipe unavailable"))?;

        if let Some(mut stdin) = child.stdin.take() {
            thread::spawn(move || {
                let _ = stdin.write_all(&stdin_bytes);
            });
        }

        Ok((Self { child, pid }, ChildPipes { stdout, stderr }))
    }

    pub fn wait_with_deadline(&mut self, deadline_unix_ms: Option<u64>) -> Value {
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status_value(status),
                Ok(None) => {}
                Err(error) => {
                    return wait_failed_status(&error);
                }
            }

            if deadline_unix_ms.is_some_and(|deadline| crate::encoding::now_unix_ms() >= deadline) {
                self.terminate_group();
                return cancelled_status();
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn terminate_group(&mut self) {
        self.signal_group(SIGTERM);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            if self.child.try_wait().ok().flatten().is_some() {
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

fn status_value(status: std::process::ExitStatus) -> Value {
    if let Some(code) = status.code() {
        return json!({ "kind": "exited", "code": code });
    }

    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        return json!({ "kind": "signal_terminated", "signal": signal });
    }

    json!({ "kind": "unknown" })
}

fn wait_failed_status(error: &io::Error) -> Value {
    json!({ "kind": "spawn_error", "reason": format!("wait failed: {error}") })
}

fn cancelled_status() -> Value {
    json!({ "kind": "cancelled" })
}
