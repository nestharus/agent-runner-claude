// declared_role: validator, accessor, parser, mapper

use serde_json::{json, Value};

use super::invoke::Invocation;

pub fn assert_successful_invocation(output: &Invocation) {
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
}

pub fn assert_single_launch_event_count(events: &[Value]) {
    assert_eq!(
        events.len(),
        1,
        "pre-spawn rejection must emit one exit event"
    );
}

pub fn first_launch_event(events: &[Value]) -> &Value {
    &events[0]
}

pub fn assert_pre_spawn_launch_exit_fields(event: &Value, reason_fragment: &str) {
    assert_eq!(event["kind"], "exit");
    assert_eq!(event["status"]["kind"], "spawn_error");
    assert!(
        event["status"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains(reason_fragment),
        "unexpected pre-spawn rejection reason: {event}"
    );
    assert_eq!(event["terminal_signal"]["kind"], "spawn_error");
}

pub fn assert_setup_brain_argv(argv: &[String], model: &str, resume: Option<&str>, prompt: &str) {
    let prefix = expected_setup_brain_prefix(model);
    assert_setup_brain_prefix(argv, &prefix);
    let schema_arg = setup_brain_schema_arg(argv, prefix.len());
    let schema = parse_setup_brain_schema(schema_arg);
    assert_setup_brain_schema_object(&schema);
    let expected = expected_setup_brain_argv(prefix, schema_arg, resume, prompt);
    assert_eq!(argv, expected.as_slice());
}

fn assert_setup_brain_prefix(argv: &[String], prefix: &[String]) {
    assert!(argv.len() > prefix.len(), "argv too short: {argv:?}");
    assert_eq!(&argv[..prefix.len()], prefix);
}

fn setup_brain_schema_arg(argv: &[String], schema_index: usize) -> &str {
    &argv[schema_index]
}

fn parse_setup_brain_schema(schema_arg: &str) -> Value {
    serde_json::from_str::<Value>(schema_arg).expect("--json-schema must be JSON")
}

fn assert_setup_brain_schema_object(schema: &Value) {
    assert!(schema.is_object());
}

fn expected_setup_brain_prefix(model: &str) -> Vec<String> {
    vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model.to_string(),
        "--allowedTools".to_string(),
        "Read,Bash,Glob,Grep".to_string(),
        "--no-session-persistence".to_string(),
        "--json-schema".to_string(),
    ]
}

fn expected_setup_brain_argv(
    mut expected: Vec<String>,
    schema_arg: &str,
    resume: Option<&str>,
    prompt: &str,
) -> Vec<String> {
    expected.push(schema_arg.to_string());
    if let Some(resume) = resume {
        expected.push("--resume".to_string());
        expected.push(resume.to_string());
    }
    expected.push(prompt.to_string());
    expected
}

pub fn assert_terminal_evidence_bounded(signal: &Value) {
    if let Some(evidence) = signal["evidence"].as_str() {
        assert!(
            evidence.len() <= 200,
            "evidence must be bounded: {evidence}"
        );
    }
}

pub fn assert_terminal_observed_at(signal: &Value, observed_at_unix_ms: u64) {
    assert_eq!(signal["observed_at_unix_ms"], json!(observed_at_unix_ms));
}
