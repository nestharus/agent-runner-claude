// declared_role: orchestration, accessor, validator, mapper, parser

mod support;

use serde_json::{json, Value};
use support::assertions::{
    assert_successful_invocation, assert_terminal_evidence_bounded, assert_terminal_observed_at,
};
use support::fixtures::{temp_roots, TempRoots};
use support::invoke::{invoke, parse_one_stdout_json, Invocation};
use support::requests::{
    terminal_classify_request, terminal_classify_request_from_base64, TERMINAL_OBSERVED_AT_UNIX_MS,
};
use support::schema::assert_valid;

fn classify(stdout: &[u8], stderr: &[u8], status: Value) -> Value {
    let roots = temp_roots("terminal");
    let request = terminal_request(&roots, stdout, stderr, status);
    let output = terminal_invocation(&request);
    assert_terminal_invocation_success(&output);
    let response = terminal_stdout_response(&output);
    assert_terminal_response_schema(&response);
    response
}

fn terminal_request(roots: &TempRoots, stdout: &[u8], stderr: &[u8], status: Value) -> Value {
    terminal_classify_request(roots, stdout, stderr, status)
}

fn terminal_invocation(request: &Value) -> Invocation {
    invoke("terminal.classify", request)
}

fn terminal_stdout_response(output: &Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn assert_terminal_invocation_success(output: &Invocation) {
    assert_successful_invocation(output);
}

fn assert_terminal_response_schema(response: &Value) {
    assert_valid(
        "terminal.schema.json#/$defs/TerminalClassifyResponse",
        response,
    );
}

fn signal_kind(response: &Value) -> &str {
    response["result"]["terminal_signal"]["kind"]
        .as_str()
        .unwrap()
}

#[test]
fn terminal_classifies_process_status_variants_and_signal_vocabulary() {
    let cases = [
        (
            json!({ "kind": "exited", "code": 0 }),
            b"".as_slice(),
            b"".as_slice(),
            "clean_exit",
        ),
        (
            json!({ "kind": "exited", "code": 7 }),
            b"".as_slice(),
            b"".as_slice(),
            "nonzero_exit",
        ),
        (
            json!({ "kind": "signal_terminated", "signal": 15 }),
            b"".as_slice(),
            b"".as_slice(),
            "signal_exit",
        ),
        (
            json!({ "kind": "spawn_error", "reason": "ENOENT" }),
            b"".as_slice(),
            b"".as_slice(),
            "spawn_error",
        ),
        (
            json!({ "kind": "prolonged_silence", "reason": "no output" }),
            b"".as_slice(),
            b"".as_slice(),
            "prolonged_silence",
        ),
        (
            json!({ "kind": "cancelled" }),
            b"".as_slice(),
            b"".as_slice(),
            "cancelled",
        ),
        (
            json!({ "kind": "unknown" }),
            b"".as_slice(),
            b"".as_slice(),
            "unknown",
        ),
        (
            json!({ "kind": "exited", "code": 1 }),
            b"Claude usage limit reached".as_slice(),
            b"".as_slice(),
            "nonzero_exit",
        ),
        (
            json!({ "kind": "exited", "code": 1 }),
            b"".as_slice(),
            b"usage limit may reset soon".as_slice(),
            "nonzero_exit",
        ),
        (
            json!({ "kind": "exited", "code": 1 }),
            b"".as_slice(),
            b"429 Too Many Requests".as_slice(),
            "nonzero_exit",
        ),
    ];

    for (status, stdout, stderr, expected) in cases {
        let response = classify(stdout, stderr, status);
        assert_eq!(signal_kind(&response), expected);
        let signal = &response["result"]["terminal_signal"];
        assert_terminal_observed_at(signal, TERMINAL_OBSERVED_AT_UNIX_MS);
        assert_terminal_evidence_bounded(signal);
    }
}

#[test]
fn terminal_invalid_base64_returns_error_response() {
    let roots = temp_roots("terminal-invalid-base64");
    let request = terminal_classify_request_from_base64(
        &roots,
        "not@@base64".to_string(),
        String::new(),
        json!({ "kind": "exited", "code": 0 }),
    );
    let output = invoke("terminal.classify", &request);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "terminal.schema.json#/$defs/TerminalClassifyErrorResponse",
        &response,
    );
    assert_eq!(response["error"]["category"], "invalid_request");
}

#[test]
fn terminal_does_not_false_match_quota_negations() {
    let response = classify(
        b"quota is not exhausted and no usage limit was reached",
        b"",
        json!({ "kind": "exited", "code": 1 }),
    );
    assert_eq!(signal_kind(&response), "nonzero_exit");
}

#[test]
fn terminal_does_not_use_quota_or_rate_limit_substrings_for_verdicts() {
    for (stdout, stderr) in [
        (b"Claude usage limit reached".as_slice(), b"".as_slice()),
        (b"usage limit may reset soon".as_slice(), b"".as_slice()),
        (b"quota exhausted".as_slice(), b"".as_slice()),
        (b"".as_slice(), b"429 Too Many Requests".as_slice()),
        (b"".as_slice(), b"rate limit exceeded".as_slice()),
    ] {
        let response = classify(stdout, stderr, json!({ "kind": "exited", "code": 1 }));
        assert_eq!(signal_kind(&response), "nonzero_exit");
        assert!(!matches!(
            signal_kind(&response),
            "quota_exhausted_inband" | "maybe_quota_exhausted" | "rate_limited"
        ));
    }
}
