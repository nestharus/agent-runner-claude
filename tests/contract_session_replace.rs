mod support;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke("session.replace", &request);
    assert!(output.stderr.is_empty());
    (output.code, parse_one_stdout_json(&output))
}

fn transcript_path(roots: &TempRoots, file: &str) -> PathBuf {
    let dir = roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work");
    fs::create_dir_all(&dir).expect("create claude project dir");
    dir.join(file)
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

#[cfg(unix)]
fn set_dir_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set directory mode");
}

fn has_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| has_string(item, needle)),
        Value::Object(map) => map.values().any(|item| has_string(item, needle)),
        _ => false,
    }
}

fn canonical_one(id: &str, text: &str) -> Vec<u8> {
    format!(
        "{{\"body\":[{{\"text\":\"{text}\",\"type\":\"text\"}}],\"id\":\"uuid:{id}\",\"role\":\"user\",\"timestamp\":\"2026-06-04T00:00:00.000Z\",\"type\":\"turn\"}}\n"
    )
    .into_bytes()
}

fn native_one(session_id: &str, uuid: &str, text: &str) -> Vec<u8> {
    format!(
        "{}\n",
        json!({
            "sessionId": session_id,
            "uuid": uuid,
            "timestamp": "2026-06-04T00:00:00.000Z",
            "type": "user",
            "message": { "role": "user", "content": text }
        })
    )
    .into_bytes()
}

fn native_records(bytes: &[u8]) -> Vec<Value> {
    parse_native_records(non_empty_native_lines(&native_transcript_text(bytes)))
}

fn native_transcript_text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("native transcript is UTF-8")
}

fn non_empty_native_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .collect()
}

fn parse_native_records(lines: Vec<&str>) -> Vec<Value> {
    lines.into_iter().map(parse_native_record).collect()
}

fn parse_native_record(line: &str) -> Value {
    serde_json::from_str::<Value>(line).expect("native JSONL record parses")
}

fn assert_single_native_record(bytes: &[u8], session_id: &str, uuid: &str, text: &str) {
    let records = native_records(bytes);
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record["sessionId"], session_id);
    assert_eq!(record["uuid"], uuid);
    assert_eq!(record["message"]["role"], "user");
    assert_eq!(record["message"]["content"], text);
}

fn replace_params(
    session_id: &str,
    path: &Path,
    canonical: &[u8],
    preimage: Option<String>,
) -> Value {
    let mut params = json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "path": path.display().to_string(),
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "canonical_transcript": { "kind": "bytes", "data_base64": encode_b64(canonical) }
    });
    if let Some(preimage) = preimage {
        params["preimage_sha256"] = json!(preimage);
    }
    params
}

#[test]
fn replace_rejects_transcript_paths_that_escape_provider_transcript_root_without_host_writes() {
    let roots = temp_roots("session-replace-host-path-confinement");
    let project_dir = roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work");
    fs::create_dir_all(&project_dir).expect("create claude project dir");

    let traversal_target = roots.home.join("host_escape.jsonl");
    let absolute_host_target = roots.data_root.join("absolute-host-transcript.jsonl");
    let host_sqlite = roots.data_root.join("central-state.sqlite");
    fs::write(&traversal_target, b"TRAVERSAL_SENTINEL").expect("write traversal sentinel");
    fs::write(&absolute_host_target, b"ABSOLUTE_HOST_SENTINEL").expect("write absolute sentinel");
    fs::write(&host_sqlite, b"HOST_SQLITE_SENTINEL").expect("write sqlite sentinel");

    let canonical = canonical_one("u-host-path", "should not be written");
    let escaped_by_traversal = project_dir.join("../../../host_escape.jsonl");
    for hostile_path in [&escaped_by_traversal, &absolute_host_target, &host_sqlite] {
        let (code, response) = call(
            &roots,
            replace_params("sess-host-path", hostile_path, &canonical, None),
        );

        assert_eq!(code, Some(2));
        assert_valid(
            "session.schema.json#/$defs/SessionReplaceErrorResponse",
            &response,
        );
        assert!(!response["ok"].as_bool().unwrap());
        assert_eq!(response["error"]["category"], "conflict");
    }

    assert_eq!(fs::read(&traversal_target).unwrap(), b"TRAVERSAL_SENTINEL");
    assert_eq!(
        fs::read(&absolute_host_target).unwrap(),
        b"ABSOLUTE_HOST_SENTINEL"
    );
    assert_eq!(fs::read(&host_sqlite).unwrap(), b"HOST_SQLITE_SENTINEL");
}

#[cfg(unix)]
#[test]
fn replace_rejects_symlinked_transcript_path_that_resolves_outside_root() {
    let roots = temp_roots("session-replace-symlink-confinement");
    let project_dir = roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work");
    fs::create_dir_all(&project_dir).expect("create claude project dir");
    let outside_transcript = roots.root.join("outside-transcript.jsonl");
    let original = native_one("sess-symlink", "u1", "outside sentinel");
    fs::write(&outside_transcript, &original).expect("write outside transcript");
    let symlink_path = project_dir.join("linked.jsonl");
    symlink(&outside_transcript, &symlink_path).expect("create transcript symlink");

    let canonical = canonical_one("u2", "should not be written through symlink");
    let (code, response) = call(
        &roots,
        replace_params("sess-symlink", &symlink_path, &canonical, None),
    );

    assert_eq!(code, Some(2));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceErrorResponse",
        &response,
    );
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "conflict");
    assert_eq!(fs::read(&outside_transcript).unwrap(), original);
}

#[test]
fn replace_rejects_invalid_canonical_input_before_touching_transcript() {
    let roots = temp_roots("session-replace-invalid");
    let path = transcript_path(&roots, "invalid.jsonl");
    let original = native_one("sess-invalid", "u1", "original");
    fs::write(&path, &original).expect("write transcript");

    let (code, response) = call(
        &roots,
        replace_params(
            "sess-invalid",
            &path,
            b"not jsonl\n",
            Some(sha256_hex(&original)),
        ),
    );

    assert_eq!(code, Some(2));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceErrorResponse",
        &response,
    );
    assert_eq!(response["error"]["category"], "invalid_request");
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn replace_preimage_mismatch_returns_conflict_and_keeps_file_byte_identical() {
    let roots = temp_roots("session-replace-conflict");
    let path = transcript_path(&roots, "conflict.jsonl");
    let original = native_one("sess-conflict", "u1", "original");
    fs::write(&path, &original).expect("write transcript");
    let canonical = canonical_one("u2", "changed");

    let (code, response) = call(
        &roots,
        replace_params("sess-conflict", &path, &canonical, Some("0".repeat(64))),
    );

    assert_eq!(code, Some(2));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceErrorResponse",
        &response,
    );
    assert_eq!(response["error"]["category"], "conflict");
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn replace_write_failure_never_exposes_partial_transcript_and_retry_hash_matches() {
    let roots = temp_roots("session-replace-write-failure");
    let path = transcript_path(&roots, "write-failure.jsonl");
    let parent = path.parent().expect("transcript path should have parent");
    let original = native_one("sess-write-failure", "u1", "original");
    fs::write(&path, &original).expect("write transcript");
    let canonical = canonical_one("u2", "after failure");
    let params = replace_params(
        "sess-write-failure",
        &path,
        &canonical,
        Some(sha256_hex(&original)),
    );

    set_dir_mode(parent, 0o500);
    let (code, response) = call(&roots, params.clone());
    set_dir_mode(parent, 0o700);

    assert_eq!(code, Some(1));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceErrorResponse",
        &response,
    );
    assert_eq!(response["error"]["category"], "failed");
    let after_failed = fs::read(&path).expect("transcript should remain readable");
    assert_eq!(after_failed, original);

    let (code, response) = call(&roots, params);
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceResponse",
        &response,
    );
    let actual_postimage = fs::read(&path).expect("postimage should be readable");
    assert_single_native_record(
        &actual_postimage,
        "sess-write-failure",
        "u2",
        "after failure",
    );
    assert_eq!(
        response["result"]["postimage_sha256"],
        sha256_hex(&actual_postimage)
    );
}

#[test]
fn replace_absent_preimage_allowed_noop_unchanged_reports_changed_false() {
    let roots = temp_roots("session-replace-noop");
    let path = transcript_path(&roots, "noop.jsonl");
    let original = native_one("sess-noop", "u1", "same");
    fs::write(&path, &original).expect("write transcript");
    let canonical = canonical_one("u1", "same");

    let (code, response) = call(&roots, replace_params("sess-noop", &path, &canonical, None));

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceResponse",
        &response,
    );
    assert!(!response["result"]["changed"].as_bool().unwrap());
    assert_eq!(response["result"]["artifacts"], json!([]));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn replace_changed_uses_atomic_native_rendering_postimage_hash_and_host_state_plan_without_host_writes(
) {
    let roots = temp_roots("session-replace-changed");
    let path = transcript_path(&roots, "changed.jsonl");
    let original = native_one("sess-changed", "u1", "before");
    fs::write(&path, &original).expect("write transcript");
    let host_db = roots.home.join(".claude").join("host.sqlite");
    let host_journal = roots.home.join(".claude").join("host.sqlite-journal");
    fs::write(&host_db, b"host-db-sentinel").expect("write host db sentinel");
    fs::write(&host_journal, b"host-journal-sentinel").expect("write host journal sentinel");

    let canonical = canonical_one("u2", "after");
    let (code, response) = call(
        &roots,
        replace_params(
            "sess-changed",
            &path,
            &canonical,
            Some(sha256_hex(&original)),
        ),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceResponse",
        &response,
    );
    let result = &response["result"];
    assert!(result["changed"].as_bool().unwrap());
    let actual_native = fs::read(&path).unwrap();
    assert_ne!(
        actual_native, original,
        "changed replace must publish changed native state"
    );
    assert_eq!(result["postimage_sha256"], sha256_hex(&actual_native));
    let actual_text =
        String::from_utf8(actual_native.clone()).expect("native transcript must be UTF-8 JSONL");
    assert!(
        actual_text.ends_with('\n'),
        "native transcript must be line-delimited JSONL"
    );
    assert!(
        !actual_text.contains("not jsonl"),
        "never leave partial transcript bytes"
    );
    let lines = actual_text.lines().collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "single-turn canonical replacement must render one native record"
    );
    let native_record =
        serde_json::from_str::<Value>(lines[0]).expect("native record must parse as JSON");
    assert_eq!(native_record["sessionId"], "sess-changed");
    assert_eq!(native_record["uuid"], "u2");
    assert!(
        has_string(&native_record, "after"),
        "native record must preserve replacement body text: {native_record}"
    );
    assert_eq!(fs::read(&host_db).unwrap(), b"host-db-sentinel");
    assert_eq!(fs::read(&host_journal).unwrap(), b"host-journal-sentinel");

    let plan = &result["host_state_plan"];
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["operation"], "session.replace");
    assert_eq!(plan["session_id"], "sess-changed");
    assert_eq!(plan["provider_name"], "claude");
    assert_eq!(plan["canonical_format"], "oulipoly.canonical_transcript/v1");
    assert_eq!(plan["turn_count"], 1);
    assert_eq!(plan["records_sha256"], sha256_hex(&canonical));
    assert_eq!(plan["postimage_sha256"], sha256_hex(&actual_native));
    assert_eq!(plan["artifacts"][0]["kind"], "transcript");
}
