mod support;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke("session.export", &request);
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

#[test]
fn export_returns_byte_deterministic_canonical_jsonl_base64_count_and_sha256() {
    let roots = temp_roots("session-export-canonical");
    let path = transcript_path(&roots, "-tmp-work", "canonical.jsonl");
    write_lines(
        &path,
        &[
            native_line("sess-export", "u-user", "user", "user", json!("hello")),
            native_line(
                "sess-export",
                "u-assistant",
                "assistant",
                "assistant",
                json!([{ "type": "text", "text": "hi" }]),
            ),
        ],
    );
    let expected = concat!(
        "{\"body\":[{\"text\":\"hello\",\"type\":\"text\"}],\"id\":\"uuid:u-user\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
        "{\"body\":[{\"text\":\"hi\",\"type\":\"text\"}],\"id\":\"uuid:u-assistant\",\"role\":\"assistant\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
    )
    .as_bytes();

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-export" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionExportResponse",
        &response,
    );
    let result = &response["result"];
    assert_eq!(
        result["canonical_format"],
        "oulipoly.canonical_transcript/v1"
    );
    assert_eq!(result["data_base64"], encode_b64(expected));
    assert_eq!(result["turn_count"], 2);
    assert_eq!(result["sha256"], sha256_hex(expected));
}

#[test]
fn export_preserves_compaction_boundary_and_skips_unsupported_records() {
    let roots = temp_roots("session-export-compaction");
    let path = transcript_path(&roots, "-tmp-work", "compact.jsonl");
    write_lines(
        &path,
        &[
            json!({
                "sessionId": "sess-compact",
                "uuid": "summary-1",
                "timestamp": "2026-06-04T00:00:00.000Z",
                "type": "system",
                "isCompactSummary": true,
                "message": { "content": "previous conversation summary" }
            })
            .to_string(),
            json!({ "sessionId": "sess-compact", "uuid": "tool-ignored", "timestamp": "2026-06-04T00:00:00.000Z", "type": "tool_result", "content": "unsupported" }).to_string(),
            native_line("sess-compact", "u-after", "user", "user", json!("after compact")),
        ],
    );
    let expected = concat!(
        "{\"body\":[{\"text\":\"previous conversation summary\",\"type\":\"text\"}],\"id\":\"uuid:summary-1\",\"role\":\"summary\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"compaction_boundary\"}\n",
        "{\"body\":[{\"text\":\"after compact\",\"type\":\"text\"}],\"id\":\"uuid:u-after\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
    )
    .as_bytes();

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-compact" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionExportResponse",
        &response,
    );
    assert_eq!(response["result"]["data_base64"], encode_b64(expected));
    assert_eq!(response["result"]["turn_count"], 1);
    assert_eq!(response["result"]["sha256"], sha256_hex(expected));
}

#[test]
fn export_zero_turn_transcript_has_empty_canonical_bytes_and_empty_sha() {
    let roots = temp_roots("session-export-zero");
    let path = transcript_path(&roots, "-tmp-work", "empty.jsonl");
    fs::write(&path, "").expect("write empty transcript");

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "empty-session", "path": path.display().to_string() }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionExportResponse",
        &response,
    );
    assert_eq!(
        response["result"]["canonical_format"],
        "oulipoly.canonical_transcript/v1"
    );
    assert_eq!(response["result"]["data_base64"], "");
    assert_eq!(response["result"]["turn_count"], 0);
    assert_eq!(response["result"]["sha256"], sha256_hex(b""));
}

#[test]
fn export_partial_malformed_transcript_returns_invalid_request_error_not_partial_bytes() {
    let roots = temp_roots("session-export-partial");
    let path = transcript_path(&roots, "-tmp-work", "partial.jsonl");
    write_lines(
        &path,
        &[
            native_line("sess-partial-export", "u-ok", "user", "user", json!("ok")),
            "{malformed".to_string(),
        ],
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-partial-export" }),
    );

    assert_eq!(code, Some(2));
    assert_valid(
        "session.schema.json#/$defs/SessionExportErrorResponse",
        &response,
    );
    assert_eq!(response["error"]["category"], "invalid_request");
}
