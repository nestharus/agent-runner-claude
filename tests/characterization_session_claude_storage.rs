// declared_role: orchestration, mapper, accessor, validator, formatter, parser
// intrinsic_surface_declarations:
//   - component: tests/characterization_session_claude_storage.rs
//     role: intrinsic-surface
//     Domain: characterization_session_claude_storage_proof_surface
//     Owns:
//       - Claude native transcript storage characterization scenarios
//       - support harness dependencies for session invoke/schema proof

mod support;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> (Option<i32>, Value) {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert_empty_stderr(&output);
    (output.code, parse_one_stdout_json(&output))
}

fn assert_empty_stderr(output: &support::invoke::Invocation) {
    assert!(output.stderr.is_empty());
}

fn transcript_path(roots: &TempRoots, project: &str, file: &str) -> PathBuf {
    let dir = transcript_dir(roots, project);
    create_transcript_dir(&dir);
    transcript_file_path(&dir, file)
}

fn transcript_dir(roots: &TempRoots, project: &str) -> PathBuf {
    roots.home.join(".claude").join("projects").join(project)
}

fn create_transcript_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("create claude project dir");
}

fn transcript_file_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(file)
}

fn write_lines(path: &Path, lines: &[String]) {
    fs::write(path, transcript_text(lines)).expect("write transcript lines");
}

fn transcript_text(lines: &[String]) -> String {
    format!("{}\n", lines.join("\n"))
}

fn native_record_value(
    session_id: &str,
    uuid: &str,
    typ: &str,
    role: &str,
    content: Value,
) -> Value {
    json!({
        "sessionId": session_id,
        "uuid": uuid,
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": typ,
        "message": { "role": role, "content": content },
        "parentUuid": null,
        "isSidechain": false
    })
}

fn record_text(record: Value) -> String {
    record.to_string()
}

fn native(session_id: &str, uuid: &str, typ: &str, role: &str, content: Value) -> String {
    record_text(native_record_value(session_id, uuid, typ, role, content))
}

fn text_content(text: &str) -> Value {
    json!(text)
}

fn message_text_content(text: &str) -> Value {
    json!({ "type": "text", "text": text })
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

fn sha256_hex(bytes: &[u8]) -> String {
    hex_text(&sha256_digest(bytes))
}

fn sha256_digest(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn hex_text(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn locate_transcript_params(session_id: &str) -> Value {
    json!({ "settings_id": "claude-primary", "session_id": session_id })
}

fn top_content_record_value() -> Value {
    json!({
        "sessionId": "fallback-session",
        "uuid": "top-content",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "user",
        "content": [{ "type": "text", "text": "top-level content" }]
    })
}

fn read_turns_params(session_id: &str) -> Value {
    json!({ "settings_id": "claude-primary", "session_id": session_id })
}

fn sidechain_export_record_value() -> Value {
    json!({
        "sessionId": "export-session",
        "uuid": "side",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "assistant",
        "isSidechain": true,
        "message": { "role": "assistant", "content": "sidechain" }
    })
}

fn unsupported_export_record_value() -> Value {
    json!({
        "sessionId": "export-session",
        "uuid": "unsupported",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "tool_use",
        "message": { "content": "unsupported" }
    })
}

fn summary_export_record_value() -> Value {
    json!({
        "sessionId": "export-session",
        "uuid": "summary",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "system",
        "isCompactSummary": true,
        "message": { "content": "summary" }
    })
}

fn export_params(session_id: &str) -> Value {
    json!({ "settings_id": "claude-primary", "session_id": session_id })
}

fn replace_params(path: String, canonical_data_base64: String, preimage_sha256: String) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": "replace-session",
        "path": path,
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "canonical_transcript": { "kind": "bytes", "data_base64": canonical_data_base64 },
        "preimage_sha256": preimage_sha256
    })
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[test]
fn claude_native_project_traversal_prefers_exact_session_id_record_not_filename() {
    let roots = temp_roots("char-session-traversal");
    let unrelated = transcript_path(&roots, "-repo-a", "target-session.jsonl");
    write_lines(
        &unrelated,
        &[native(
            "other",
            "other-u",
            "user",
            "user",
            text_content("wrong"),
        )],
    );
    let expected = transcript_path(&roots, "-repo-b", "random-name.jsonl");
    write_lines(
        &expected,
        &[native(
            "target-session",
            "target-u",
            "user",
            "user",
            text_content("right"),
        )],
    );

    let (code, response) = call(
        &roots,
        "session.locate_transcript",
        locate_transcript_params("target-session"),
    );
    assert_locate_response(code, &response, &expected);
}

fn assert_locate_response(code: Option<i32>, response: &Value, expected: &Path) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        response,
    );
    assert!(response["result"]["located"].as_bool().unwrap());
    assert_eq!(response["result"]["path"], expected.display().to_string());
}

#[test]
fn claude_native_content_fallback_and_turn_normalization_are_pinned() {
    let roots = temp_roots("char-session-content-fallback");
    let path = transcript_path(&roots, "-repo", "fallback.jsonl");
    write_lines(
        &path,
        &[
            record_text(top_content_record_value()),
            native(
                "fallback-session",
                "message-content",
                "assistant",
                "assistant",
                message_text_content("message object"),
            ),
        ],
    );

    let (code, response) = call(
        &roots,
        "session.read_turns",
        read_turns_params("fallback-session"),
    );
    assert_read_turns_response(code, &response);
}

fn assert_read_turns_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        response,
    );
    let turns = response["result"]["turns"].as_array().unwrap();
    assert_eq!(
        turns[0]["body"],
        json!([{ "type": "text", "text": "top-level content" }])
    );
    assert_eq!(
        turns[1]["body"],
        json!([{ "type": "text", "text": "message object" }])
    );
}

#[test]
fn claude_native_canonical_export_fixture_and_sha_skip_sidechain_and_unsupported() {
    let roots = temp_roots("char-session-export");
    let path = transcript_path(&roots, "-repo", "export.jsonl");
    write_lines(
        &path,
        &[
            native(
                "export-session",
                "u1",
                "user",
                "user",
                text_content("first"),
            ),
            record_text(sidechain_export_record_value()),
            record_text(unsupported_export_record_value()),
            record_text(summary_export_record_value()),
            native(
                "export-session",
                "u2",
                "assistant",
                "assistant",
                text_content("second"),
            ),
        ],
    );
    let expected = expected_canonical_export_bytes();

    let (code, response) = call(&roots, "session.export", export_params("export-session"));
    assert_export_response(code, &response, expected);
}

fn expected_canonical_export_bytes() -> &'static [u8] {
    concat!(
        "{\"body\":[{\"text\":\"first\",\"type\":\"text\"}],\"id\":\"uuid:u1\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
        "{\"body\":[{\"text\":\"summary\",\"type\":\"text\"}],\"id\":\"uuid:summary\",\"role\":\"summary\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"compaction_boundary\"}\n",
        "{\"body\":[{\"text\":\"second\",\"type\":\"text\"}],\"id\":\"uuid:u2\",\"role\":\"assistant\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
    )
    .as_bytes()
}

fn assert_export_response(code: Option<i32>, response: &Value, expected: &[u8]) {
    assert_eq!(code, Some(0));
    assert_valid("session.schema.json#/$defs/SessionExportResponse", response);
    assert_eq!(
        response["result"]["canonical_format"],
        "oulipoly.canonical_transcript/v1"
    );
    assert_eq!(response["result"]["data_base64"], encode_b64(expected));
    assert_eq!(response["result"]["sha256"], sha256_hex(expected));
    assert_eq!(response["result"]["turn_count"], 2);
}

#[test]
fn claude_native_replace_renders_back_to_jsonl_record_shape() {
    let roots = temp_roots("char-session-replace-render");
    let path = transcript_path(&roots, "-repo", "replace.jsonl");
    let original = original_replace_transcript();
    fs::write(&path, original.as_bytes()).expect("write transcript");
    let canonical = canonical_replace_transcript();

    let (code, response) = call(
        &roots,
        "session.replace",
        replace_params(
            display_path(&path),
            encode_b64(canonical),
            sha256_hex(original.as_bytes()),
        ),
    );
    assert_replace_response(code, &response, &path);
}

fn original_replace_transcript() -> String {
    format!(
        "{}\n",
        native(
            "replace-session",
            "old",
            "user",
            "user",
            text_content("old")
        )
    )
}

fn canonical_replace_transcript() -> &'static [u8] {
    b"{\"body\":[{\"text\":\"new\",\"type\":\"text\"}],\"id\":\"uuid:new\",\"role\":\"assistant\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n"
}

fn assert_replace_response(code: Option<i32>, response: &Value, path: &Path) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceResponse",
        response,
    );
    assert_rendered_replace_record(path);
}

fn assert_rendered_replace_record(path: &Path) {
    let record = rendered_first_record(path);
    assert_eq!(record["sessionId"], "replace-session");
    assert_eq!(record["uuid"], "new");
    assert_eq!(record["type"], "assistant");
    assert_eq!(record["message"]["role"], "assistant");
    assert_eq!(record["message"]["content"], "new");
}

fn rendered_first_record(path: &Path) -> Value {
    parse_first_record(&rendered_transcript_text(path))
}

fn rendered_transcript_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read rendered transcript")
}

fn parse_first_record(rendered: &str) -> Value {
    serde_json::from_str::<Value>(first_line(rendered)).expect("native JSONL record parses")
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap()
}
