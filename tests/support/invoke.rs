use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

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
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-runner-claude"));
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().expect("spawn provider binary");
    child
        .stdin
        .as_mut()
        .expect("provider stdin")
        .write_all(stdin)
        .expect("write provider stdin");

    let output = child.wait_with_output().expect("wait for provider binary");
    Invocation {
        stdout: output.stdout,
        stderr: output.stderr,
        code: output.status.code(),
    }
}

pub fn parse_one_stdout_json(invocation: &Invocation) -> Value {
    let mut stream = serde_json::Deserializer::from_slice(&invocation.stdout).into_iter::<Value>();
    let value = stream
        .next()
        .expect("stdout must contain one JSON value")
        .expect("stdout JSON value must parse");
    assert!(
        stream.next().is_none(),
        "stdout must contain exactly one JSON value: {}",
        String::from_utf8_lossy(&invocation.stdout)
    );
    value
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
