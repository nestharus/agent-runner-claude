// declared_role: orchestration, accessor, validator

mod support;

use serde_json::{json, Value};
use support::assertions::assert_successful_invocation;
use support::fixtures::temp_roots;
use support::invoke::{invoke, parse_one_stdout_json};
use support::requests::terminal_classify_request;
use support::schema::assert_valid;

fn classify(stdout: &[u8], stderr: &[u8], status: Value) -> Value {
    let roots = temp_roots("terminal-characterization");
    let request = terminal_classify_request(&roots, stdout, stderr, status);
    let output = invoke("terminal.classify", &request);
    assert_successful_invocation(&output);
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "terminal.schema.json#/$defs/TerminalClassifyResponse",
        &response,
    );
    response
}

fn kind(response: &Value) -> &str {
    response["result"]["terminal_signal"]["kind"]
        .as_str()
        .unwrap()
}

#[test]
fn claude_terminal_clean_exit_precedes_quota_text() {
    let response = classify(
        b"",
        b"Claude usage limit reached",
        json!({ "kind": "exited", "code": 0 }),
    );
    assert_terminal_kind(&response, "clean_exit");
}

#[test]
fn claude_terminal_spawn_error_and_cancelled_status_precede_output_text() {
    let spawn = classify(
        b"Claude usage limit reached",
        b"429 Too Many Requests",
        json!({ "kind": "spawn_error", "reason": "No such file" }),
    );
    assert_terminal_kind(&spawn, "spawn_error");

    let cancelled = classify(
        b"Claude usage limit reached",
        b"429 Too Many Requests",
        json!({ "kind": "cancelled" }),
    );
    assert_terminal_kind(&cancelled, "cancelled");
}

fn assert_terminal_kind(response: &Value, expected: &str) {
    assert_eq!(kind(response), expected);
}

#[test]
fn claude_terminal_evidence_is_bounded_and_uses_contract_vocabulary() {
    let long = vec![b'x'; 4096];
    let response = classify(
        &long,
        b"Claude usage limit reached",
        json!({ "kind": "exited", "code": 1 }),
    );
    assert_terminal_vocabulary_and_evidence(&response);
}

fn assert_terminal_vocabulary_and_evidence(response: &Value) {
    let signal = &response["result"]["terminal_signal"];
    assert!(matches!(
        kind(response),
        "clean_exit"
            | "nonzero_exit"
            | "signal_exit"
            | "spawn_error"
            | "prolonged_silence"
            | "cancelled"
            | "unknown"
    ));
    if let Some(evidence) = signal["evidence"].as_str() {
        assert!(evidence.len() <= 200);
    }
}
