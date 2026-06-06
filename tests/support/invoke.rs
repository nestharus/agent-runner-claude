// declared_role: orchestration, mapper, parser, validator, formatter
// adapter_declarations:
//   - component: tests/support/invoke.rs
//     role: adapter
//     Translates:
//       - provider binary process contract (argv/stdin/stdout/stderr/exit)
//       - contract/v1/common.schema.json#/$defs/SuccessResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorResponseEnvelope
//       - contract/v1/launch.schema.json JSONL event stream

use serde_json::Value;
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Output, Stdio};

#[derive(Debug)]
pub struct Invocation {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub code: Option<i32>,
}

pub fn invoke(subcommand: &str, envelope: &Value) -> Invocation {
    invoke_with_stdin_bytes(Some(subcommand), envelope.to_string().as_bytes())
}

pub fn invoke_with_stdin_bytes(subcommand: Option<&str>, stdin: &[u8]) -> Invocation {
    let args = subcommand.into_iter().collect::<Vec<_>>();
    invoke_with_args_and_stdin_bytes(&args, stdin)
}

pub fn invoke_with_args_and_stdin_bytes(args: &[&str], stdin: &[u8]) -> Invocation {
    let child = spawn_provider_process(args);
    let output = run_provider_process(child, stdin);
    invocation_from_output(output)
}

fn spawn_provider_process(args: &[&str]) -> Child {
    provider_command(args)
        .spawn()
        .expect("spawn provider binary")
}

fn provider_command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run_provider_process(mut child: Child, stdin: &[u8]) -> Output {
    write_stdin(provider_stdin(&mut child), stdin);
    child.wait_with_output().expect("wait for provider binary")
}

fn provider_stdin(child: &mut Child) -> &mut ChildStdin {
    child.stdin.as_mut().expect("provider stdin")
}

fn write_stdin(target: &mut ChildStdin, stdin: &[u8]) {
    target.write_all(stdin).expect("write provider stdin");
}

fn invocation_from_output(output: Output) -> Invocation {
    Invocation {
        stdout: output.stdout,
        stderr: output.stderr,
        code: output.status.code(),
    }
}

pub fn parse_one_stdout_json(invocation: &Invocation) -> Value {
    let values = stdout_json_values(&invocation.stdout);
    assert_one_stdout_json_value(&values, &invocation.stdout);
    values
        .into_iter()
        .next()
        .expect("stdout must contain one JSON value")
}

fn stdout_json_values(stdout: &[u8]) -> Vec<Value> {
    serde_json::Deserializer::from_slice(stdout)
        .into_iter::<Value>()
        .map(|value| value.expect("stdout JSON value must parse"))
        .collect()
}

fn assert_one_stdout_json_value(values: &[Value], stdout: &[u8]) {
    assert!(!values.is_empty(), "stdout must contain one JSON value");
    assert!(
        values.len() == 1,
        "stdout must contain exactly one JSON value: {}",
        lossy_stdout(stdout)
    );
}

fn lossy_stdout(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout).to_string()
}

pub fn collect_launch_jsonl_lines(invocation: &Invocation) -> Vec<Value> {
    let stdout = launch_stdout_text(invocation);
    parse_launch_jsonl_lines(non_empty_jsonl_lines(&stdout))
}

fn launch_stdout_text(invocation: &Invocation) -> String {
    String::from_utf8(invocation.stdout.clone()).expect("launch stdout must be UTF-8 JSONL")
}

fn non_empty_jsonl_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect()
}

fn parse_launch_jsonl_lines(lines: Vec<&str>) -> Vec<Value> {
    lines
        .into_iter()
        .map(|line| serde_json::from_str::<Value>(line).expect("launch JSONL line must parse"))
        .collect()
}
