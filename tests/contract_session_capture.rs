// declared_role: orchestration, mapper, formatter, parser, validator, predicate, accessor

mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let output = invoke_session_capture(&capture_request(roots, params));
    assert_empty_stderr(&output);
    (output.code, stdout_json(&output))
}

fn capture_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn invoke_session_capture(request: &Value) -> support::invoke::Invocation {
    invoke("session.capture", request)
}

fn assert_empty_stderr(output: &support::invoke::Invocation) {
    assert!(output.stderr.is_empty());
}

fn stdout_json(output: &support::invoke::Invocation) -> Value {
    parse_one_stdout_json(output)
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

fn capture_none_params() -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": "logical-session",
        "strategy": "none"
    })
}

fn forced_flag_readback_params(transcript_path: &Path) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": "logical-session",
        "strategy": "forced_flag_readback",
        "transcript_path": transcript_path.display().to_string()
    })
}

fn stdout_json_event_params(stdout: &[u8]) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": "logical-session",
        "strategy": "stdout_json_event",
        "stdout_base64": encode_b64(stdout)
    })
}

fn start_known_params(stdout: &[u8]) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": "logical-session",
        "strategy": "start_known",
        "provider_session_id": "known-native-789",
        "stdout_base64": encode_b64(stdout)
    })
}

fn malformed_capture_params() -> Value {
    json!({})
}

fn capture_project_dir(roots: &TempRoots) -> PathBuf {
    roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work")
}

fn prepared_capture_project_dir(roots: &TempRoots) -> PathBuf {
    let dir = capture_project_dir(roots);
    create_capture_project_dir(&dir);
    dir
}

fn create_capture_project_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("create transcript dir");
}

fn capture_project_file(dir: &Path, file: &str) -> PathBuf {
    dir.join(file)
}

fn capture_transcript_path(roots: &TempRoots, file: &str) -> PathBuf {
    let dir = prepared_capture_project_dir(roots);
    capture_project_file(&dir, file)
}

fn forced_flag_transcript(roots: &TempRoots) -> PathBuf {
    let transcript = capture_transcript_path(roots, "forced.jsonl");
    write_forced_flag_transcript(&transcript);
    transcript
}

fn write_forced_flag_transcript(transcript: &Path) {
    fs::write(transcript, forced_flag_transcript_text()).expect("write transcript");
}

fn forced_flag_transcript_text() -> &'static str {
    r#"{"type":"claude_session_capture_event","session_id":"forced-native-123"}
"#
}

fn traversal_capture_target(roots: &TempRoots) -> PathBuf {
    roots.home.join("hostile-capture.jsonl")
}

fn absolute_capture_target(roots: &TempRoots) -> PathBuf {
    roots.data_root.join("absolute-hostile-capture.jsonl")
}

fn escaped_capture_path(project_dir: &Path) -> PathBuf {
    project_dir.join("../../../hostile-capture.jsonl")
}

fn write_hostile_capture_sentinels(traversal_target: &Path, absolute_target: &Path) {
    write_traversal_capture_sentinel(traversal_target);
    write_absolute_capture_sentinel(absolute_target);
}

fn write_traversal_capture_sentinel(path: &Path) {
    fs::write(path, traversal_capture_sentinel_text()).expect("write traversal sentinel");
}

fn write_absolute_capture_sentinel(path: &Path) {
    fs::write(path, absolute_capture_sentinel_text()).expect("write absolute sentinel");
}

fn traversal_capture_sentinel_text() -> &'static str {
    "claude_session_capture_event=traversal-should-not-read\n"
}

fn absolute_capture_sentinel_text() -> &'static str {
    "claude_session_capture_event=absolute-should-not-read\n"
}

fn assert_hostile_capture_sentinels(traversal_target: &Path, absolute_target: &Path) {
    assert_capture_file_text(traversal_target, traversal_capture_sentinel_text());
    assert_capture_file_text(absolute_target, absolute_capture_sentinel_text());
}

fn assert_capture_file_text(path: &Path, expected: &str) {
    assert_eq!(capture_file_text(path), expected);
}

fn capture_file_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[cfg(unix)]
fn symlink_capture_target(roots: &TempRoots) -> PathBuf {
    roots.root.join("outside-capture.jsonl")
}

#[cfg(unix)]
fn linked_capture_path(project_dir: &Path) -> PathBuf {
    project_dir.join("linked.jsonl")
}

#[cfg(unix)]
fn write_symlink_capture_sentinel(path: &Path) {
    fs::write(path, symlink_capture_sentinel_bytes()).expect("write outside sentinel");
}

#[cfg(unix)]
fn symlink_capture_sentinel_bytes() -> &'static [u8] {
    b"claude_session_capture_event=symlink-should-not-read\n"
}

#[cfg(unix)]
fn create_capture_symlink(outside: &Path, linked: &Path) {
    symlink(outside, linked).expect("create symlink");
}

#[cfg(unix)]
fn assert_symlink_capture_sentinel(path: &Path) {
    assert_eq!(capture_file_bytes(path), symlink_capture_sentinel_bytes());
}

#[cfg(unix)]
fn capture_file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn stdout_session_event() -> &'static [u8] {
    br#"noise
{"type":"system","subtype":"init","session_id":"stdout-native-456","cwd":"/tmp"}
more noise
"#
}

fn stdout_camel_case_session_event() -> &'static [u8] {
    br#"{"type":"system","subtype":"init","sessionId":"stdout-native-camel-456"}
"#
}

fn synthetic_capture_sentinel_stdout() -> &'static [u8] {
    br#"{"type":"claude_session_capture_event","subtype":"init","session_id":"sentinel-should-not-capture"}
"#
}

fn start_known_stdout_event() -> &'static [u8] {
    br#"{"type":"system","session_id":"stdout-should-not-win"}"#
}

fn assert_capture_none_response(code: Option<i32>, response: &Value) {
    assert_success_capture_response(code, response);
    let result = &response["result"];
    assert!(result["provider_session_id"].is_null());
    assert_eq!(result["artifacts"], json!([]));
    assert_eq!(
        result["state"],
        json!({ "kind": "not_captured", "strategy": "none" })
    );
}

fn assert_forced_flag_capture_response(code: Option<i32>, response: &Value, transcript: &Path) {
    assert_success_capture_response(code, response);
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

fn assert_forced_flag_conflict(roots: &TempRoots, hostile_path: &Path) {
    let (code, response) = call(roots, forced_flag_readback_params(hostile_path));
    assert_capture_error(code, &response, 1, "conflict");
}

fn assert_stdout_event_capture_response(
    code: Option<i32>,
    response: &Value,
    expected_session_id: &str,
) {
    assert_success_capture_response(code, response);
    let result = &response["result"];
    assert_eq!(result["provider_session_id"], expected_session_id);
    assert_eq!(result["state"]["source"], "stdout_json_event");
    assert_eq!(result["artifacts"][0]["kind"], "stdout_event");
}

fn assert_stdout_sentinel_ignored_response(code: Option<i32>, response: &Value) {
    assert_success_capture_response(code, response);
    let result = &response["result"];
    assert!(result["provider_session_id"].is_null());
    assert!(result["state"]["provider_session_id"].is_null());
    assert_eq!(result["state"]["source"], "stdout_json_event");
}

fn assert_start_known_capture_response(code: Option<i32>, response: &Value) {
    assert_success_capture_response(code, response);
    let result = &response["result"];
    assert_eq!(result["provider_session_id"], "known-native-789");
    assert_eq!(result["state"]["source"], "start_known");
}

fn assert_success_capture_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionCaptureResponse",
        response,
    );
}

fn assert_capture_error(code: Option<i32>, response: &Value, expected_code: i32, category: &str) {
    assert_eq!(code, Some(expected_code));
    assert_error_response(response, category);
}

#[test]
fn capture_none_strategy_returns_schema_valid_absence_projection() {
    let roots = temp_roots("session-capture-none");

    let (code, response) = call(&roots, capture_none_params());
    assert_capture_none_response(code, &response);
}

#[test]
fn capture_forced_flag_readback_projects_provider_session_artifact_and_state() {
    let roots = temp_roots("session-capture-forced");
    let transcript = forced_flag_transcript(&roots);

    let (code, response) = call(&roots, forced_flag_readback_params(&transcript));
    assert_forced_flag_capture_response(code, &response, &transcript);
}

#[test]
fn capture_forced_flag_readback_rejects_absolute_and_traversal_escapes() {
    let roots = temp_roots("session-capture-hostile-paths");
    let project_dir = prepared_capture_project_dir(&roots);
    let traversal_target = traversal_capture_target(&roots);
    let absolute_target = absolute_capture_target(&roots);
    write_hostile_capture_sentinels(&traversal_target, &absolute_target);

    let escaped_by_traversal = escaped_capture_path(&project_dir);
    for hostile_path in [&escaped_by_traversal, &absolute_target] {
        assert_forced_flag_conflict(&roots, hostile_path);
    }
    assert_hostile_capture_sentinels(&traversal_target, &absolute_target);
}

#[cfg(unix)]
#[test]
fn capture_forced_flag_readback_rejects_symlink_escape() {
    let roots = temp_roots("session-capture-symlink-escape");
    let project_dir = prepared_capture_project_dir(&roots);
    let outside = symlink_capture_target(&roots);
    write_symlink_capture_sentinel(&outside);
    let linked = linked_capture_path(&project_dir);
    create_capture_symlink(&outside, &linked);

    assert_forced_flag_conflict(&roots, &linked);
    assert_symlink_capture_sentinel(&outside);
}

#[test]
fn capture_stdout_json_event_extracts_provider_session_id_and_artifact() {
    let roots = temp_roots("session-capture-stdout-event");

    let (code, response) = call(&roots, stdout_json_event_params(stdout_session_event()));
    assert_stdout_event_capture_response(code, &response, "stdout-native-456");
}

#[test]
fn capture_stdout_json_event_accepts_camel_case_session_id() {
    let roots = temp_roots("session-capture-stdout-event-camel-case");

    let (code, response) = call(
        &roots,
        stdout_json_event_params(stdout_camel_case_session_event()),
    );
    assert_stdout_event_capture_response(code, &response, "stdout-native-camel-456");
}

#[test]
fn capture_stdout_json_event_ignores_synthetic_capture_sentinel() {
    let roots = temp_roots("session-capture-stdout-sentinel-ignored");

    let (code, response) = call(
        &roots,
        stdout_json_event_params(synthetic_capture_sentinel_stdout()),
    );
    assert_stdout_sentinel_ignored_response(code, &response);
}

#[test]
fn capture_start_known_provider_session_id_takes_precedence_over_output_guessing() {
    let roots = temp_roots("session-capture-start-known");

    let (code, response) = call(&roots, start_known_params(start_known_stdout_event()));
    assert_start_known_capture_response(code, &response);
}

#[test]
fn capture_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("session-capture-malformed-request");

    let (code, response) = call(&roots, malformed_capture_params());
    assert_capture_error(code, &response, 2, "invalid_request");
}
