mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke("session.read_turns", &request);
    assert!(output.stderr.is_empty());
    (output.code, parse_one_stdout_json(&output))
}

fn transcript_path(roots: &TempRoots, project: &str, file: &str) -> PathBuf {
    let dir = roots.home.join(".claude").join("projects").join(project);
    fs::create_dir_all(&dir).expect("create claude project dir");
    dir.join(file)
}

fn write_lines(path: &Path, lines: &[String]) {
    fs::write(path, format!("{}\n", lines.join("\n"))).expect("write transcript lines");
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

#[test]
fn read_turns_normalizes_roles_body_variants_and_stable_ids_from_uuid_or_line() {
    let roots = temp_roots("session-read-turns");
    let path = transcript_path(&roots, "-tmp-work", "conversation.jsonl");
    write_lines(
        &path,
        &[
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
        ],
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-read" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        &response,
    );
    assert!(response["result"]["complete"].as_bool().unwrap());
    assert_eq!(response["result"]["turn_count"], 3);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_eq!(turns[0]["id"], "uuid:u-user");
    assert_eq!(turns[0]["role"], "user");
    assert_eq!(
        turns[0]["body"],
        json!([{ "type": "text", "text": "hello" }])
    );
    assert_eq!(turns[1]["id"], "uuid:u-assistant");
    assert_eq!(turns[1]["role"], "assistant");
    assert_eq!(turns[1]["body"], json!([{ "type": "text", "text": "hi" }]));
    assert_eq!(turns[2]["id"], "line:3");
    assert_eq!(
        turns[2]["body"],
        json!([{ "type": "text", "text": "fallback" }])
    );
}

#[test]
fn read_turns_zero_turn_transcript_is_complete_empty_result() {
    let roots = temp_roots("session-read-zero");
    let path = transcript_path(&roots, "-tmp-work", "empty.jsonl");
    fs::write(&path, "").expect("write empty transcript");

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "empty-session", "path": path.display().to_string() }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        &response,
    );
    assert_eq!(response["result"]["turn_count"], 0);
    assert_eq!(response["result"]["turns"], json!([]));
    assert!(response["result"]["complete"].as_bool().unwrap());
}

#[test]
fn read_turns_malformed_jsonl_is_partial_not_silent_success() {
    let roots = temp_roots("session-read-partial");
    let path = transcript_path(&roots, "-tmp-work", "partial.jsonl");
    write_lines(
        &path,
        &[
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
        ],
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-partial" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        &response,
    );
    assert!(!response["result"]["complete"].as_bool().unwrap());
    assert_eq!(response["result"]["turn_count"], 1);
    assert_eq!(response["result"]["turns"][0]["id"], "uuid:u-ok");
}

#[test]
fn read_turns_after_turn_id_filters_strictly_after_stable_turn() {
    let roots = temp_roots("session-read-after-turn");
    let path = transcript_path(&roots, "-tmp-work", "after.jsonl");
    write_lines(
        &path,
        &[
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
        ],
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-after", "after_turn_id": "uuid:u1" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        &response,
    );
    assert_eq!(response["result"]["turn_count"], 2);
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_eq!(turns[0]["id"], "uuid:u2");
    assert_eq!(turns[1]["id"], "uuid:u3");
}

#[test]
fn read_turns_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("session-read-malformed-request");

    let (code, response) = call(&roots, json!({}));

    assert_eq!(code, Some(2));
    assert_error_response(&response, "invalid_request");
}
