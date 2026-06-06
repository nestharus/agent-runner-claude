// declared_role: accessor, filter, mapper, orchestration, predicate, validator
// adapter_declarations:
//   - component: src/external/shell.rs
//     role: adapter
//     Translates:
//       - POSIX child process contract (argv/env/stdin/stdout/stderr/exit/timeout)

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[derive(Debug, Clone)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub stdin: Vec<u8>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

pub fn run_command(request: &CommandRequest) -> io::Result<CommandOutput> {
    let mut command = process_command(request);
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&request.stdin)?;
    }

    let started = Instant::now();
    let mut timed_out = false;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if timeout_elapsed(started, request.timeout) {
            timed_out = true;
            let _ = kill_child_tree(&mut child);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let output = child.wait_with_output()?;
    Ok(command_output(output, timed_out))
}

fn process_command(request: &CommandRequest) -> Command {
    let mut command = Command::new(&request.program);
    command
        .args(&request.args)
        // hermetic: declared env only, inherited env cleared
        .env_clear()
        .envs(&request.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    command
}

fn timeout_elapsed(started: Instant, timeout: Duration) -> bool {
    started.elapsed() >= timeout
}

#[cfg(unix)]
fn kill_child_tree(child: &mut Child) -> io::Result<()> {
    let pgid = -(child.id() as i32);
    let result = unsafe { kill(pgid, SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        child.kill()
    }
}

#[cfg(not(unix))]
fn kill_child_tree(child: &mut Child) -> io::Result<()> {
    child.kill()
}

fn command_output(output: Output, timed_out: bool) -> CommandOutput {
    CommandOutput {
        status_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_request(script: &str, timeout: Duration) -> CommandRequest {
        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), "/usr/bin:/bin".to_string());

        CommandRequest {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env,
            cwd: None,
            stdin: Vec::new(),
            timeout,
        }
    }

    #[test]
    fn captures_stdout_stderr_and_nonzero_status() {
        let output = run_command(&fixture_request(
            "printf 'stdout fixture'; printf 'stderr fixture' >&2; exit 7",
            Duration::from_secs(2),
        ))
        .expect("fixture command should run");

        assert_eq!(output.status_code, Some(7));
        assert_eq!(output.stdout, b"stdout fixture");
        assert_eq!(output.stderr, b"stderr fixture");
        assert!(!output.timed_out);
    }

    #[test]
    fn reports_bounded_timeout_without_waiting_for_inherited_pipes() {
        let started = Instant::now();
        let output = run_command(&fixture_request("sleep 2", Duration::from_millis(10)))
            .expect("timeout fixture should still produce output status");

        assert!(output.timed_out);
        assert_eq!(output.status_code, None);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "timeout path waited for descendant-held pipes"
        );
    }
}
