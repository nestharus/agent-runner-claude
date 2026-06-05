// declared_role: accessor, orchestration, mapper
// adapter_declarations:
//   - component: src/external/claude_cli.rs
//     role: adapter
//     Translates:
//       - claude CLI process contract (argv/stdin/stdout/stderr/exit)

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use super::shell::{run_command, CommandOutput, CommandRequest};

pub fn run_claude(
    args: Vec<String>,
    stdin: Vec<u8>,
    env: BTreeMap<String, String>,
    timeout: Duration,
) -> io::Result<CommandOutput> {
    run_command(&claude_command_request(args, stdin, env, timeout))
}

pub fn auth_status(env: BTreeMap<String, String>, timeout: Duration) -> io::Result<CommandOutput> {
    run_claude(auth_status_args(), Vec::new(), env, timeout)
}

fn claude_command_request(
    args: Vec<String>,
    stdin: Vec<u8>,
    env: BTreeMap<String, String>,
    timeout: Duration,
) -> CommandRequest {
    CommandRequest {
        program: "claude".to_string(),
        args,
        env,
        cwd: None,
        stdin,
        timeout,
    }
}

fn auth_status_args() -> Vec<String> {
    vec!["auth".to_string(), "status".to_string()]
}
