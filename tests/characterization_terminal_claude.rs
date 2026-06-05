mod support;

use serde_json::{json, Value};
use support::fixtures::{envelope, host_context, temp_roots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn classify(stdout: &[u8], stderr: &[u8], status: Value) -> Value {
    let roots = temp_roots("terminal-characterization");
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
    assert_eq!(kind(&response), "clean_exit");
}

#[test]
fn claude_terminal_spawn_error_and_cancelled_status_precede_output_text() {
    let spawn = classify(
        b"Claude usage limit reached",
        b"429 Too Many Requests",
        json!({ "kind": "spawn_error", "reason": "No such file" }),
    );
    assert_eq!(kind(&spawn), "spawn_error");

    let cancelled = classify(
        b"Claude usage limit reached",
        b"429 Too Many Requests",
        json!({ "kind": "cancelled" }),
    );
    assert_eq!(kind(&cancelled), "cancelled");
}

#[test]
fn claude_terminal_evidence_is_bounded_and_uses_contract_vocabulary() {
    let long = vec![b'x'; 4096];
    let response = classify(
        &long,
        b"Claude usage limit reached",
        json!({ "kind": "exited", "code": 1 }),
    );
    let signal = &response["result"]["terminal_signal"];
    assert!(matches!(
        kind(&response),
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
