mod support;

use serde_json::{json, Value};
use support::fixtures::{envelope, host_context, temp_roots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn classify(stdout: &[u8], stderr: &[u8], status: Value) -> Value {
    let roots = temp_roots("terminal");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({
            "stdout_base64": encode_b64(stdout),
            "stderr_base64": encode_b64(stderr),
            "status": status,
            "observed_at_unix_ms": 1779991000200u64
        }),
    );
    let output = invoke("terminal.classify", &request);
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(
        "terminal.schema.json#/$defs/TerminalClassifyResponse",
        &response,
    );
    response
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
        assert_eq!(signal["observed_at_unix_ms"], 1779991000200u64);
        if let Some(evidence) = signal["evidence"].as_str() {
            assert!(
                evidence.len() <= 200,
                "evidence must be bounded: {evidence}"
            );
        }
    }
}

#[test]
fn terminal_invalid_base64_returns_error_response() {
    let roots = temp_roots("terminal-invalid-base64");
    let request = envelope(
        CONTRACT,
        host_context(&roots),
        json!({
            "stdout_base64": "not@@base64",
            "stderr_base64": "",
            "status": { "kind": "exited", "code": 0 },
            "observed_at_unix_ms": 1779991000200u64
        }),
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

fn encode_b64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}
