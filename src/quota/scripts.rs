// declared_role: orchestration, mapper
// adapter_declarations:
//   - component: src/quota/scripts.rs
//     role: adapter
//     Translates:
//       - contract/v1/quota.schema.json#/$defs/QuotaProbeRequest
//       - contract/v1/quota.schema.json#/$defs/QuotaRefreshAuthRequest
//       - scripts/anthropic-usage process contract (argv/stdout/stderr/exit)
//       - configured quota refresh command process contract (argv/stdout/stderr/exit)

use std::collections::BTreeMap;
use std::time::Duration;

use crate::external::shell::{run_command, CommandOutput, CommandRequest};

pub fn run_quota_script(script: &str) -> std::io::Result<CommandOutput> {
    run_command(&quota_script_request(script))
}

fn quota_script_request(script: &str) -> CommandRequest {
    CommandRequest {
        program: script.to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        stdin: Vec::new(),
        timeout: Duration::from_secs(5),
    }
}

pub fn run_refresh_command(command: &str) -> std::io::Result<CommandOutput> {
    run_command(&refresh_command_request(command))
}

fn refresh_command_request(command: &str) -> CommandRequest {
    CommandRequest {
        program: command.to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        cwd: None,
        stdin: Vec::new(),
        timeout: Duration::from_secs(10),
    }
}
