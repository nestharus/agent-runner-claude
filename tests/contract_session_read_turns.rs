// declared_role: orchestration, mapper, formatter, validator, accessor
// intrinsic_surface_declarations:
//   - component: tests/contract_session_read_turns.rs
//     role: intrinsic-surface
//     Domain: contract_session_read_turns_proof_surface
//     Owns:
//       - session read-turns contract scenarios
//       - support harness dependencies for session invoke/schema proof

mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json, Invocation};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let output = invoke_session_read_turns(roots, params);
    assert_empty_stderr(&output);
    (output.code, parse_one_stdout_json(&output))
}

fn invoke_session_read_turns(roots: &TempRoots, params: Value) -> Invocation {
    let request = contract_request(roots, params);
    invoke("session.read_turns", &request)
}

fn contract_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn assert_empty_stderr(output: &Invocation) {
    assert!(output.stderr.is_empty());
}

fn transcript_dir_path(roots: &TempRoots, project: &str) -> PathBuf {
    roots.home.join(".claude").join("projects").join(project)
}

fn transcript_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(file)
}

fn create_transcript_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("create claude project dir");
}

fn prepared_transcript_path(roots: &TempRoots, project: &str, file: &str) -> PathBuf {
    let dir = transcript_dir_path(roots, project);
    create_transcript_dir(&dir);
    transcript_path(&dir, file)
}

fn lines_text(lines: &[String]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn write_lines(path: &Path, lines: &[String]) {
    let text = lines_text(lines);
    write_text(path, &text);
}

fn write_text(path: &Path, text: &str) {
    fs::write(path, text).expect("write transcript lines");
}

fn write_empty_transcript(path: &Path) {
    fs::write(path, "").expect("write empty transcript");
}

fn assert_error_response(value: &Value, category: &str) {
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsErrorResponse",
        value,
    );
    assert!(!value["ok"].as_bool().unwrap());
    assert_eq!(value["error"]["category"], category);
}

fn native_line(
    session_id: &str,
    uuid: Option<&str>,
    typ: &str,
    role: Option<&str>,
    content: Value,
) -> String {
    let mut record = json!({
        "sessionId": session_id,
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": typ,
        "message": { "content": content }
    });
    if let Some(uuid) = uuid {
        record["uuid"] = json!(uuid);
    }
    if let Some(role) = role {
        record["message"]["role"] = json!(role);
    }
    record.to_string()
}

fn native_turn_line_without_session_id(
    uuid: &str,
    typ: &str,
    role: &str,
    content: Value,
) -> String {
    json!({
        "uuid": uuid,
        "parentUuid": "parent-turn",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": typ,
        "message": { "role": role, "content": content }
    })
    .to_string()
}

fn native_metadata_line(session_id: &str, typ: &str) -> String {
    json!({
        "sessionId": session_id,
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": typ
    })
    .to_string()
}

fn normalized_turn_lines() -> Vec<String> {
    vec![
        native_line(
            "sess-read",
            Some("u-user"),
            "user",
            Some("user"),
            json!("hello"),
        ),
        native_line(
            "sess-read",
            Some("u-assistant"),
            "assistant",
            Some("assistant"),
            json!([{ "type": "text", "text": "hi" }]),
        ),
        native_line(
            "sess-read",
            None,
            "user",
            Some("user"),
            json!({ "type": "text", "text": "fallback" }),
        ),
    ]
}

fn claude_turn_lines_without_per_line_session_id() -> Vec<String> {
    vec![
        native_metadata_line("sess-native-no-turn-sid", "queue-operation"),
        native_turn_line_without_session_id("u-native-user", "user", "user", json!("hello")),
        native_metadata_line("sess-native-no-turn-sid", "last-prompt"),
        native_turn_line_without_session_id(
            "u-native-assistant",
            "assistant",
            "assistant",
            json!([{ "type": "text", "text": "hi" }]),
        ),
    ]
}

fn partial_turn_lines() -> Vec<String> {
    vec![
        native_line(
            "sess-partial",
            Some("u-ok"),
            "user",
            Some("user"),
            json!("first"),
        ),
        "{not valid json".to_string(),
        native_line(
            "sess-partial",
            Some("u-after"),
            "assistant",
            Some("assistant"),
            json!("after malformed"),
        ),
    ]
}

fn after_turn_lines() -> Vec<String> {
    vec![
        native_line("sess-after", Some("u1"), "user", Some("user"), json!("one")),
        native_line(
            "sess-after",
            Some("u2"),
            "assistant",
            Some("assistant"),
            json!("two"),
        ),
        native_line(
            "sess-after",
            Some("u3"),
            "user",
            Some("user"),
            json!("three"),
        ),
    ]
}

fn invalid_timestamp_lines() -> Vec<String> {
    vec![json!({
        "sessionId": "sess-invalid-timestamp",
        "uuid": "u-invalid-timestamp",
        "timestamp": "not-rfc3339",
        "type": "user",
        "message": { "role": "user", "content": "bad timestamp" }
    })
    .to_string()]
}

fn write_normalized_turns_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "conversation.jsonl");
    write_lines(&path, &normalized_turn_lines());
}

fn write_claude_turns_without_per_line_session_id_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "native-no-turn-sid.jsonl");
    write_lines(&path, &claude_turn_lines_without_per_line_session_id());
}

fn write_zero_turn_fixture(roots: &TempRoots) -> PathBuf {
    let path = prepared_transcript_path(roots, "-tmp-work", "empty.jsonl");
    write_empty_transcript(&path);
    path
}

fn write_partial_turns_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "partial.jsonl");
    write_lines(&path, &partial_turn_lines());
}

fn write_after_turn_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "after.jsonl");
    write_lines(&path, &after_turn_lines());
}

fn write_invalid_timestamp_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "invalid-timestamp.jsonl");
    write_lines(&path, &invalid_timestamp_lines());
}

fn read_turns_request(session_id: &str) -> Value {
    json!({ "settings_id": "claude-primary", "session_id": session_id })
}

fn read_turns_path_request(session_id: &str, path: &str) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "path": path
    })
}

fn read_turns_after_request(session_id: &str, after_turn_id: &str) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "after_turn_id": after_turn_id
    })
}

fn malformed_request() -> Value {
    json!({})
}

fn assert_text_body(value: &Value, text: &str) {
    let body = value.as_array().expect("turn body array");
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["type"], "text");
    assert_eq!(body[0]["text"], text);
}

fn assert_host_turn_shape(turn: &Value, turn_id: &str, session_id: &str, role: &str) {
    assert!(turn.get("id").is_none());
    assert_eq!(turn["turn_id"], turn_id);
    assert_eq!(turn["session_id"], session_id);
    assert_eq!(turn["role"], role);
    assert_parseable_timestamp(turn["timestamp"].as_str().expect("turn timestamp string"));
}

fn assert_parseable_timestamp(timestamp: &str) {
    assert!(!timestamp.is_empty());
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("turn timestamp parses as RFC3339");
}

fn assert_normalized_turns_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert!(response["result"]["complete"].as_bool().unwrap());
    assert_eq!(response["result"]["turn_count"], 3);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_turns_session_id(turns, "sess-read");
    assert_host_turn_shape(&turns[0], "uuid:u-user", "sess-read", "user");
    assert_text_body(&turns[0]["body"], "hello");
    assert_host_turn_shape(&turns[1], "uuid:u-assistant", "sess-read", "assistant");
    assert_text_body(&turns[1]["body"], "hi");
    assert_host_turn_shape(&turns[2], "line:3", "sess-read", "user");
    assert_text_body(&turns[2]["body"], "fallback");
}

fn assert_turns_session_id(turns: &[Value], expected: &str) {
    for turn in turns {
        assert_eq!(turn["session_id"], expected);
    }
}

fn assert_claude_turns_without_per_line_session_id_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert!(response["result"]["complete"].as_bool().unwrap());
    assert_eq!(response["result"]["turn_count"], 2);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_turns_session_id(turns, "sess-native-no-turn-sid");
    assert_host_turn_shape(
        &turns[0],
        "uuid:u-native-user",
        "sess-native-no-turn-sid",
        "user",
    );
    assert_eq!(turns[0]["parent_turn_id"], "uuid:parent-turn");
    assert_text_body(&turns[0]["body"], "hello");
    assert_host_turn_shape(
        &turns[1],
        "uuid:u-native-assistant",
        "sess-native-no-turn-sid",
        "assistant",
    );
    assert_eq!(turns[1]["parent_turn_id"], "uuid:parent-turn");
    assert_text_body(&turns[1]["body"], "hi");
}

fn assert_zero_turn_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert_eq!(response["result"]["turn_count"], 0);
    assert!(response["result"]["turns"].as_array().unwrap().is_empty());
    assert!(response["result"]["complete"].as_bool().unwrap());
}

fn assert_partial_turns_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert!(!response["result"]["complete"].as_bool().unwrap());
    assert_eq!(response["result"]["turn_count"], 1);
    assert_eq!(response["result"]["turns"][0]["turn_id"], "uuid:u-ok");
}

fn assert_after_turn_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert_eq!(response["result"]["turn_count"], 2);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_eq!(turns[0]["turn_id"], "uuid:u2");
    assert_eq!(turns[1]["turn_id"], "uuid:u3");
}

fn assert_invalid_timestamp_fallback_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    let turn = &response["result"]["turns"][0];
    assert_host_turn_shape(
        turn,
        "uuid:u-invalid-timestamp",
        "sess-invalid-timestamp",
        "user",
    );
    assert_eq!(turn["timestamp"], "1970-01-01T00:00:00Z");
}

fn assert_malformed_request_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(2));
    assert_error_response(response, "invalid_request");
}

#[test]
fn read_turns_normalizes_roles_body_variants_and_stable_ids_from_uuid_or_line() {
    let roots = temp_roots("session-read-turns");
    write_normalized_turns_fixture(&roots);

    let (code, response) = call(&roots, read_turns_request("sess-read"));

    assert_normalized_turns_response(code, &response);
}

#[test]
fn read_turns_stamps_requested_session_id_on_claude_turns_without_per_line_session_id() {
    let roots = temp_roots("session-read-native-no-turn-sid");
    write_claude_turns_without_per_line_session_id_fixture(&roots);

    let (code, response) = call(&roots, read_turns_request("sess-native-no-turn-sid"));

    assert_claude_turns_without_per_line_session_id_response(code, &response);
}

#[test]
fn read_turns_zero_turn_transcript_is_complete_empty_result() {
    let roots = temp_roots("session-read-zero");
    let path = write_zero_turn_fixture(&roots);
    let path = path_text(&path);

    let (code, response) = call(&roots, read_turns_path_request("empty-session", &path));

    assert_zero_turn_response(code, &response);
}

#[test]
fn read_turns_malformed_jsonl_is_partial_not_silent_success() {
    let roots = temp_roots("session-read-partial");
    write_partial_turns_fixture(&roots);

    let (code, response) = call(&roots, read_turns_request("sess-partial"));

    assert_partial_turns_response(code, &response);
}

#[test]
fn read_turns_after_turn_id_filters_strictly_after_stable_turn() {
    let roots = temp_roots("session-read-after-turn");
    write_after_turn_fixture(&roots);

    let (code, response) = call(&roots, read_turns_after_request("sess-after", "uuid:u1"));

    assert_after_turn_response(code, &response);
}

#[test]
fn read_turns_falls_back_to_parseable_timestamp_for_invalid_native_timestamp() {
    let roots = temp_roots("session-read-invalid-timestamp");
    write_invalid_timestamp_fixture(&roots);

    let (code, response) = call(&roots, read_turns_request("sess-invalid-timestamp"));

    assert_invalid_timestamp_fallback_response(code, &response);
}

#[test]
fn read_turns_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("session-read-malformed-request");

    let (code, response) = call(&roots, malformed_request());

    assert_malformed_request_response(code, &response);
}
