mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke("session.capture", &request);
    assert!(output.stderr.is_empty());
    (output.code, parse_one_stdout_json(&output))
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
        encoded.push(if chunk.len() > 1 {
            TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            TABLE[(b2 & 0b0011_1111) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

fn assert_error_response(value: &Value, category: &str) {
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureErrorResponse",
        value,
    );
    assert!(!value["ok"].as_bool().unwrap());
    assert_eq!(value["error"]["category"], category);
}

#[test]
fn capture_none_strategy_returns_schema_valid_absence_projection() {
    let roots = temp_roots("session-capture-none");

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "none"
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        &response,
    );
    let result = &response["result"];
    assert!(result["provider_session_id"].is_null());
    assert_eq!(result["artifacts"], json!([]));
    assert_eq!(
        result["state"],
        json!({ "kind": "not_captured", "strategy": "none" })
    );
}

#[test]
fn capture_forced_flag_readback_projects_provider_session_artifact_and_state() {
    let roots = temp_roots("session-capture-forced");
    let transcript = roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work")
        .join("forced.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).expect("create transcript dir");
    fs::write(
        &transcript,
        r#"{"type":"claude_session_capture_event","session_id":"forced-native-123"}
"#,
    )
    .expect("write transcript");

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "forced_flag_readback",
            "transcript_path": transcript.display().to_string()
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        &response,
    );
    let result = &response["result"];
    assert_eq!(result["provider_session_id"], "forced-native-123");
    assert_eq!(result["state"]["provider_session_id"], "forced-native-123");
    assert_eq!(result["state"]["source"], "forced_flag_readback");
    assert_eq!(result["artifacts"][0]["kind"], "transcript");
    assert_eq!(
        result["artifacts"][0]["path"],
        transcript.display().to_string()
    );
}

#[test]
fn capture_forced_flag_readback_rejects_absolute_and_traversal_escapes() {
    let roots = temp_roots("session-capture-hostile-paths");
    let project_dir = roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work");
    fs::create_dir_all(&project_dir).expect("create transcript dir");
    let traversal_target = roots.home.join("hostile-capture.jsonl");
    let absolute_target = roots.data_root.join("absolute-hostile-capture.jsonl");
    fs::write(
        &traversal_target,
        r#"claude_session_capture_event=traversal-should-not-read
"#,
    )
    .expect("write traversal sentinel");
    fs::write(
        &absolute_target,
        r#"claude_session_capture_event=absolute-should-not-read
"#,
    )
    .expect("write absolute sentinel");

    let escaped_by_traversal = project_dir.join("../../../hostile-capture.jsonl");
    for hostile_path in [&escaped_by_traversal, &absolute_target] {
        let (code, response) = call(
            &roots,
            json!({
                "settings_id": "claude-primary",
                "session_id": "logical-session",
                "strategy": "forced_flag_readback",
                "transcript_path": hostile_path.display().to_string()
            }),
        );

        assert_eq!(code, Some(2));
        assert_error_response(&response, "conflict");
    }
    assert_eq!(
        fs::read_to_string(&traversal_target).unwrap(),
        "claude_session_capture_event=traversal-should-not-read\n"
    );
    assert_eq!(
        fs::read_to_string(&absolute_target).unwrap(),
        "claude_session_capture_event=absolute-should-not-read\n"
    );
}

#[cfg(unix)]
#[test]
fn capture_forced_flag_readback_rejects_symlink_escape() {
    let roots = temp_roots("session-capture-symlink-escape");
    let project_dir = roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work");
    fs::create_dir_all(&project_dir).expect("create transcript dir");
    let outside = roots.root.join("outside-capture.jsonl");
    fs::write(
        &outside,
        b"claude_session_capture_event=symlink-should-not-read\n",
    )
    .expect("write outside sentinel");
    let linked = project_dir.join("linked.jsonl");
    symlink(&outside, &linked).expect("create symlink");

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "forced_flag_readback",
            "transcript_path": linked.display().to_string()
        }),
    );

    assert_eq!(code, Some(2));
    assert_error_response(&response, "conflict");
    assert_eq!(
        fs::read(&outside).unwrap(),
        b"claude_session_capture_event=symlink-should-not-read\n"
    );
}

#[test]
fn capture_stdout_json_event_extracts_provider_session_id_and_artifact() {
    let roots = temp_roots("session-capture-stdout-event");
    let stdout = br#"noise
{"type":"system","subtype":"init","session_id":"stdout-native-456","cwd":"/tmp"}
more noise
"#;

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "stdout_json_event",
            "stdout_base64": encode_b64(stdout)
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        &response,
    );
    let result = &response["result"];
    assert_eq!(result["provider_session_id"], "stdout-native-456");
    assert_eq!(result["state"]["source"], "stdout_json_event");
    assert_eq!(result["artifacts"][0]["kind"], "stdout_event");
}

#[test]
fn capture_stdout_json_event_accepts_camel_case_session_id() {
    let roots = temp_roots("session-capture-stdout-event-camel-case");
    let stdout = br#"{"type":"system","subtype":"init","sessionId":"stdout-native-camel-456"}
"#;

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "stdout_json_event",
            "stdout_base64": encode_b64(stdout)
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        &response,
    );
    let result = &response["result"];
    assert_eq!(result["provider_session_id"], "stdout-native-camel-456");
    assert_eq!(result["state"]["source"], "stdout_json_event");
    assert_eq!(result["artifacts"][0]["kind"], "stdout_event");
}

#[test]
fn capture_stdout_json_event_ignores_synthetic_capture_sentinel() {
    let roots = temp_roots("session-capture-stdout-sentinel-ignored");
    let stdout = br#"{"type":"claude_session_capture_event","subtype":"init","session_id":"sentinel-should-not-capture"}
"#;

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "stdout_json_event",
            "stdout_base64": encode_b64(stdout)
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        &response,
    );
    let result = &response["result"];
    assert!(result["provider_session_id"].is_null());
    assert!(result["state"]["provider_session_id"].is_null());
    assert_eq!(result["state"]["source"], "stdout_json_event");
}

#[test]
fn capture_start_known_provider_session_id_takes_precedence_over_output_guessing() {
    let roots = temp_roots("session-capture-start-known");
    let stdout = br#"{"type":"system","session_id":"stdout-should-not-win"}"#;

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "logical-session",
            "strategy": "start_known",
            "provider_session_id": "known-native-789",
            "stdout_base64": encode_b64(stdout)
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        &response,
    );
    let result = &response["result"];
    assert_eq!(result["provider_session_id"], "known-native-789");
    assert_eq!(result["state"]["source"], "start_known");
}

#[test]
fn capture_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("session-capture-malformed-request");

    let (code, response) = call(&roots, json!({}));

    assert_eq!(code, Some(2));
    assert_error_response(&response, "invalid_request");
}
