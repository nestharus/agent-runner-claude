// declared_role: accessor, mapper, orchestration
// adapter_declarations:
//   - component: src/setup/brain_cli.rs
//     role: adapter
//     Translates:
//       - contract/v1/setup.schema.json#/$defs/SetupBrainTurnRequest
//       - contract/v1/setup.schema.json#/$defs/SetupBrainTurnResult
//       - claude -p process contract (argv/stdin/stdout/stderr/exit)

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use crate::external::shell::CommandOutput;

pub fn default_setup_brain_model() -> &'static str {
    "claude-sonnet-4-6"
}

pub fn run_setup_brain(
    model: &str,
    schema: &str,
    resume: Option<&str>,
    prompt: &str,
    env: BTreeMap<String, String>,
) -> io::Result<CommandOutput> {
    crate::external::claude_cli::run_claude(
        setup_brain_args(model, schema, resume, prompt),
        setup_brain_stdin(),
        env,
        setup_brain_timeout(),
    )
}

fn setup_brain_stdin() -> Vec<u8> {
    Vec::new()
}

fn setup_brain_timeout() -> Duration {
    Duration::from_secs(30)
}

fn setup_brain_args(model: &str, schema: &str, resume: Option<&str>, prompt: &str) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--allowedTools".to_string(),
        "Read,Bash,Glob,Grep".to_string(),
        "--no-session-persistence".to_string(),
        "--json-schema".to_string(),
        schema.to_string(),
    ];
    if let Some(resume) = resume {
        args.push("--resume".to_string());
        args.push(resume.to_string());
    }
    args.push(prompt.to_string());
    args
}
