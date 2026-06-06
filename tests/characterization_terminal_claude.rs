// declared_role: orchestration, accessor, validator, mapper
// intrinsic_surface_declarations:
//   - component: tests/characterization_terminal_claude.rs
//     role: intrinsic-surface
//     Domain: characterization_terminal_claude_proof_surface
//     Owns:
//       - Claude terminal characterization scenarios
//       - support harness dependencies for terminal request/invoke/schema proof

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

fn clean_exit_status() -> Value {
    json!({ "kind": "exited", "code": 0 })
}

fn spawn_error_status() -> Value {
    json!({ "kind": "spawn_error", "reason": "No such file" })
}

fn cancelled_status() -> Value {
    json!({ "kind": "cancelled" })
}

fn nonzero_exit_status() -> Value {
    json!({ "kind": "exited", "code": 1 })
}

fn long_stdout_fixture() -> Vec<u8> {
    vec![b'x'; 4096]
}

#[test]
fn claude_terminal_clean_exit_precedes_quota_text() {
    let response = classify(b"", b"Claude usage limit reached", clean_exit_status());
    assert_terminal_kind(&response, "clean_exit");
}

#[test]
fn claude_terminal_spawn_error_and_cancelled_status_precede_output_text() {
    let spawn = classify(
        b"Claude usage limit reached",
        b"429 Too Many Requests",
        spawn_error_status(),
    );
    assert_terminal_kind(&spawn, "spawn_error");

    let cancelled = classify(
        b"Claude usage limit reached",
        b"429 Too Many Requests",
        cancelled_status(),
    );
    assert_terminal_kind(&cancelled, "cancelled");
}

fn assert_terminal_kind(response: &Value, expected: &str) {
    assert_eq!(kind(response), expected);
}

#[test]
fn claude_terminal_evidence_is_bounded_and_uses_contract_vocabulary() {
    let long = long_stdout_fixture();
    let response = classify(&long, b"Claude usage limit reached", nonzero_exit_status());
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
