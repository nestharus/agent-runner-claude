// declared_role: orchestration, mapper, formatter, validator, accessor
// intrinsic_surface_declarations:
//   - component: tests/contract_session_export.rs
//     role: intrinsic-surface
//     Domain: contract_session_export_proof_surface
//     Owns:
//       - session export contract scenarios
//       - support harness dependencies for session invoke/schema proof

mod support;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json, Invocation};
use support::schema::assert_valid;

struct ExpectedExport {
    data_base64: String,
    turn_count: i64,
    sha256: String,
}

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let output = invoke_session_export(roots, params);
    assert_empty_stderr(&output);
    (output.code, parse_one_stdout_json(&output))
}

fn invoke_session_export(roots: &TempRoots, params: Value) -> Invocation {
    let request = contract_request(roots, params);
    invoke("session.export", &request)
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

fn native_line(session_id: &str, uuid: &str, typ: &str, role: &str, content: Value) -> String {
    json!({
        "sessionId": session_id,
        "uuid": uuid,
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": typ,
        "message": { "role": role, "content": content }
    })
    .to_string()
}

fn compaction_summary_line() -> String {
    json!({
        "sessionId": "sess-compact",
        "uuid": "summary-1",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "system",
        "isCompactSummary": true,
        "message": { "content": "previous conversation summary" }
    })
    .to_string()
}

fn unsupported_tool_line() -> String {
    json!({
        "sessionId": "sess-compact",
        "uuid": "tool-ignored",
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "tool_result",
        "content": "unsupported"
    })
    .to_string()
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
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn expected_export(bytes: &[u8], turn_count: i64) -> ExpectedExport {
    ExpectedExport {
        data_base64: encode_b64(bytes),
        turn_count,
        sha256: sha256_hex(bytes),
    }
}

fn canonical_expected_export() -> ExpectedExport {
    expected_export(canonical_expected_bytes(), 2)
}

fn compaction_expected_export() -> ExpectedExport {
    expected_export(compaction_expected_bytes(), 1)
}

fn empty_expected_export() -> ExpectedExport {
    expected_export(b"", 0)
}

fn canonical_expected_bytes() -> &'static [u8] {
    concat!(
        "{\"body\":[{\"text\":\"hello\",\"type\":\"text\"}],\"id\":\"uuid:u-user\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
        "{\"body\":[{\"text\":\"hi\",\"type\":\"text\"}],\"id\":\"uuid:u-assistant\",\"role\":\"assistant\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
    )
    .as_bytes()
}

fn compaction_expected_bytes() -> &'static [u8] {
    concat!(
        "{\"body\":[{\"text\":\"previous conversation summary\",\"type\":\"text\"}],\"id\":\"uuid:summary-1\",\"role\":\"summary\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"compaction_boundary\"}\n",
        "{\"body\":[{\"text\":\"after compact\",\"type\":\"text\"}],\"id\":\"uuid:u-after\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
    )
    .as_bytes()
}

fn canonical_native_lines() -> Vec<String> {
    vec![
        native_line("sess-export", "u-user", "user", "user", json!("hello")),
        native_line(
            "sess-export",
            "u-assistant",
            "assistant",
            "assistant",
            json!([{ "type": "text", "text": "hi" }]),
        ),
    ]
}

fn compaction_native_lines() -> Vec<String> {
    vec![
        compaction_summary_line(),
        unsupported_tool_line(),
        native_line(
            "sess-compact",
            "u-after",
            "user",
            "user",
            json!("after compact"),
        ),
    ]
}

fn partial_native_lines() -> Vec<String> {
    vec![
        native_line("sess-partial-export", "u-ok", "user", "user", json!("ok")),
        "{malformed".to_string(),
    ]
}

fn write_canonical_export_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "canonical.jsonl");
    write_lines(&path, &canonical_native_lines());
}

fn write_compaction_export_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "compact.jsonl");
    write_lines(&path, &compaction_native_lines());
}

fn write_zero_export_fixture(roots: &TempRoots) -> PathBuf {
    let path = prepared_transcript_path(roots, "-tmp-work", "empty.jsonl");
    write_empty_transcript(&path);
    path
}

fn write_partial_export_fixture(roots: &TempRoots) {
    let path = prepared_transcript_path(roots, "-tmp-work", "partial.jsonl");
    write_lines(&path, &partial_native_lines());
}

fn export_request(session_id: &str) -> Value {
    json!({ "settings_id": "claude-primary", "session_id": session_id })
}

fn export_path_request(session_id: &str, path: &str) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "path": path
    })
}

fn assert_export_with_format_response(
    code: Option<i32>,
    response: &Value,
    expected: &ExpectedExport,
) {
    assert_eq!(code, Some(0));
    assert_valid("session.schema.json#/$defs/SessionExportResponse", response);
    let result = &response["result"];
    assert_eq!(
        result["canonical_format"],
        "oulipoly.canonical_transcript/v1"
    );
    assert_expected_export_fields(result, expected);
}

fn assert_export_payload_response(code: Option<i32>, response: &Value, expected: &ExpectedExport) {
    assert_eq!(code, Some(0));
    assert_valid("session.schema.json#/$defs/SessionExportResponse", response);
    assert_expected_export_fields(&response["result"], expected);
}

fn assert_expected_export_fields(result: &Value, expected: &ExpectedExport) {
    assert_eq!(
        result["data_base64"].as_str().unwrap(),
        expected.data_base64.as_str()
    );
    assert_eq!(result["turn_count"].as_i64().unwrap(), expected.turn_count);
    assert_eq!(result["sha256"].as_str().unwrap(), expected.sha256.as_str());
}

fn assert_export_error_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(2));
    assert_valid(
        "session.schema.json#/$defs/SessionExportErrorResponse",
        response,
    );
    assert_eq!(response["error"]["category"], "invalid_request");
}

#[test]
fn export_returns_byte_deterministic_canonical_jsonl_base64_count_and_sha256() {
    let roots = temp_roots("session-export-canonical");
    write_canonical_export_fixture(&roots);
    let expected = canonical_expected_export();

    let (code, response) = call(&roots, export_request("sess-export"));

    assert_export_with_format_response(code, &response, &expected);
}

#[test]
fn export_preserves_compaction_boundary_and_skips_unsupported_records() {
    let roots = temp_roots("session-export-compaction");
    write_compaction_export_fixture(&roots);
    let expected = compaction_expected_export();

    let (code, response) = call(&roots, export_request("sess-compact"));

    assert_export_payload_response(code, &response, &expected);
}

#[test]
fn export_zero_turn_transcript_has_empty_canonical_bytes_and_empty_sha() {
    let roots = temp_roots("session-export-zero");
    let path = write_zero_export_fixture(&roots);
    let path = path_text(&path);
    let expected = empty_expected_export();

    let (code, response) = call(&roots, export_path_request("empty-session", &path));

    assert_export_with_format_response(code, &response, &expected);
}

#[test]
fn export_partial_malformed_transcript_returns_invalid_request_error_not_partial_bytes() {
    let roots = temp_roots("session-export-partial");
    write_partial_export_fixture(&roots);

    let (code, response) = call(&roots, export_request("sess-partial-export"));

    assert_export_error_response(code, &response);
}
