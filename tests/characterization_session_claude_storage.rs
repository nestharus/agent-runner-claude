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

fn native(session_id: &str, uuid: &str, typ: &str, role: &str, content: Value) -> String {
    json!({
        "sessionId": session_id,
        "uuid": uuid,
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": typ,
        "message": { "role": role, "content": content },
        "parentUuid": null,
        "isSidechain": false
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
fn claude_native_project_traversal_prefers_exact_session_id_record_not_filename() {
    let roots = temp_roots("char-session-traversal");
    let unrelated = transcript_path(&roots, "-repo-a", "target-session.jsonl");
    write_lines(
        &unrelated,
        &[native("other", "other-u", "user", "user", json!("wrong"))],
    );
    let expected = transcript_path(&roots, "-repo-b", "random-name.jsonl");
    write_lines(
        &expected,
        &[native(
            "target-session",
            "target-u",
            "user",
            "user",
            json!("right"),
        )],
    );

    let (code, response) = call(
        &roots,
        "session.locate_transcript",
        json!({ "settings_id": "claude-primary", "session_id": "target-session" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        &response,
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
            json!({
                "sessionId": "fallback-session",
                "uuid": "top-content",
                "timestamp": "2026-06-04T00:00:00.000Z",
                "type": "user",
                "content": [{ "type": "text", "text": "top-level content" }]
            })
            .to_string(),
            native(
                "fallback-session",
                "message-content",
                "assistant",
                "assistant",
                json!({ "type": "text", "text": "message object" }),
            ),
        ],
    );

    let (code, response) = call(
        &roots,
        "session.read_turns",
        json!({ "settings_id": "claude-primary", "session_id": "fallback-session" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReadTurnsResponse",
        &response,
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
            native("export-session", "u1", "user", "user", json!("first")),
            json!({ "sessionId": "export-session", "uuid": "side", "timestamp": "2026-06-04T00:00:00.000Z", "type": "assistant", "isSidechain": true, "message": { "role": "assistant", "content": "sidechain" } }).to_string(),
            json!({ "sessionId": "export-session", "uuid": "unsupported", "timestamp": "2026-06-04T00:00:00.000Z", "type": "tool_use", "message": { "content": "unsupported" } }).to_string(),
            json!({ "sessionId": "export-session", "uuid": "summary", "timestamp": "2026-06-04T00:00:00.000Z", "type": "system", "isCompactSummary": true, "message": { "content": "summary" } }).to_string(),
            native("export-session", "u2", "assistant", "assistant", json!("second")),
        ],
    );
    let expected = concat!(
        "{\"body\":[{\"text\":\"first\",\"type\":\"text\"}],\"id\":\"uuid:u1\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
        "{\"body\":[{\"text\":\"summary\",\"type\":\"text\"}],\"id\":\"uuid:summary\",\"role\":\"summary\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"compaction_boundary\"}\n",
        "{\"body\":[{\"text\":\"second\",\"type\":\"text\"}],\"id\":\"uuid:u2\",\"role\":\"assistant\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n",
    )
    .as_bytes();

    let (code, response) = call(
        &roots,
        "session.export",
        json!({ "settings_id": "claude-primary", "session_id": "export-session" }),
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
    assert_eq!(response["result"]["data_base64"], encode_b64(expected));
    assert_eq!(response["result"]["sha256"], sha256_hex(expected));
    assert_eq!(response["result"]["turn_count"], 2);
}

#[test]
fn claude_native_replace_renders_back_to_jsonl_record_shape() {
    let roots = temp_roots("char-session-replace-render");
    let path = transcript_path(&roots, "-repo", "replace.jsonl");
    let original = format!(
        "{}\n",
        native("replace-session", "old", "user", "user", json!("old"))
    );
    fs::write(&path, original.as_bytes()).expect("write transcript");
    let canonical = b"{\"body\":[{\"text\":\"new\",\"type\":\"text\"}],\"id\":\"uuid:new\",\"role\":\"assistant\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}\n";

    let (code, response) = call(
        &roots,
        "session.replace",
        json!({
            "settings_id": "claude-primary",
            "session_id": "replace-session",
            "path": path.display().to_string(),
            "canonical_format": "oulipoly.canonical_transcript/v1",
            "canonical_transcript": { "kind": "bytes", "data_base64": encode_b64(canonical) },
            "preimage_sha256": sha256_hex(original.as_bytes())
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceResponse",
        &response,
    );
    let rendered = fs::read_to_string(&path).expect("read rendered transcript");
    let record = serde_json::from_str::<Value>(rendered.lines().next().unwrap())
        .expect("native JSONL record parses");
    assert_eq!(record["sessionId"], "replace-session");
    assert_eq!(record["uuid"], "new");
    assert_eq!(record["type"], "assistant");
    assert_eq!(record["message"]["role"], "assistant");
    assert_eq!(record["message"]["content"], "new");
}
