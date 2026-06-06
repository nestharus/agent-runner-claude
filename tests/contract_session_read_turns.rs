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

fn write_normalized_turns_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "conversation.jsonl");
    write_lines(&path, &normalized_turn_lines());
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

fn assert_normalized_turns_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert!(response["result"]["complete"].as_bool().unwrap());
    assert_eq!(response["result"]["turn_count"], 3);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_eq!(turns[0]["id"], "uuid:u-user");
    assert_eq!(turns[0]["role"], "user");
    assert_text_body(&turns[0]["body"], "hello");
    assert_eq!(turns[1]["id"], "uuid:u-assistant");
    assert_eq!(turns[1]["role"], "assistant");
    assert_text_body(&turns[1]["body"], "hi");
    assert_eq!(turns[2]["id"], "line:3");
    assert_text_body(&turns[2]["body"], "fallback");
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
    assert_eq!(response["result"]["turns"][0]["id"], "uuid:u-ok");
}

fn assert_after_turn_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    assert_eq!(response["result"]["turn_count"], 2);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_eq!(turns[0]["id"], "uuid:u2");
    assert_eq!(turns[1]["id"], "uuid:u3");
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
fn read_turns_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("session-read-malformed-request");

    let (code, response) = call(&roots, malformed_request());

    assert_malformed_request_response(code, &response);
}
