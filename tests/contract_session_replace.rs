// declared_role: orchestration, formatter, parser, validator, accessor, predicate, mapper

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
use support::requests::session_replace_params;
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let output = invoke_session_replace(&replace_request(roots, params));
    assert_empty_stderr(&output);
    (output.code, stdout_json(&output))
}

fn replace_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn invoke_session_replace(request: &Value) -> support::invoke::Invocation {
    invoke("session.replace", request)
}

fn assert_empty_stderr(output: &support::invoke::Invocation) {
    assert!(output.stderr.is_empty());
}

fn stdout_json(output: &support::invoke::Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn transcript_path(roots: &TempRoots, file: &str) -> PathBuf {
    let dir = prepared_transcript_dir(roots);
    transcript_file_path(&dir, file)
}

fn prepared_transcript_dir(roots: &TempRoots) -> PathBuf {
    let dir = transcript_dir(roots);
    create_transcript_dir(&dir);
    dir
}

fn transcript_dir(roots: &TempRoots) -> PathBuf {
    roots
        .home
        .join(".claude")
        .join("projects")
        .join("-tmp-work")
}

fn create_transcript_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("create claude project dir");
}

fn transcript_file_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(file)
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

fn host_path_rejection_canonical() -> Vec<u8> {
    canonical_one("u-host-path", "should not be written")
}

#[cfg(unix)]
fn symlink_original_native() -> Vec<u8> {
    native_one("sess-symlink", "u1", "outside sentinel")
}

#[cfg(unix)]
fn symlink_rejection_canonical() -> Vec<u8> {
    canonical_one("u2", "should not be written through symlink")
}

fn invalid_original_native() -> Vec<u8> {
    native_one("sess-invalid", "u1", "original")
}

fn invalid_canonical_input() -> &'static [u8] {
    b"not jsonl\n"
}

fn conflict_original_native() -> Vec<u8> {
    native_one("sess-conflict", "u1", "original")
}

fn conflict_canonical() -> Vec<u8> {
    canonical_one("u2", "changed")
}

#[cfg(unix)]
fn write_failure_original_native() -> Vec<u8> {
    native_one("sess-write-failure", "u1", "original")
}

#[cfg(unix)]
fn write_failure_canonical() -> Vec<u8> {
    canonical_one("u2", "after failure")
}

fn noop_original_native() -> Vec<u8> {
    native_one("sess-noop", "u1", "same")
}

fn noop_canonical() -> Vec<u8> {
    canonical_one("u1", "same")
}

fn changed_original_native() -> Vec<u8> {
    native_one("sess-changed", "u1", "before")
}

fn changed_canonical() -> Vec<u8> {
    canonical_one("u2", "after")
}

fn host_escape_target(roots: &TempRoots) -> PathBuf {
    roots.home.join("host_escape.jsonl")
}

fn absolute_host_transcript(roots: &TempRoots) -> PathBuf {
    roots.data_root.join("absolute-host-transcript.jsonl")
}

fn host_sqlite_path(roots: &TempRoots) -> PathBuf {
    roots.data_root.join("central-state.sqlite")
}

fn escaped_host_transcript(project_dir: &Path) -> PathBuf {
    project_dir.join("../../../host_escape.jsonl")
}

#[cfg(unix)]
fn outside_transcript_path(roots: &TempRoots) -> PathBuf {
    roots.root.join("outside-transcript.jsonl")
}

#[cfg(unix)]
fn linked_transcript_path(project_dir: &Path) -> PathBuf {
    project_dir.join("linked.jsonl")
}

fn host_db_path(roots: &TempRoots) -> PathBuf {
    roots.home.join(".claude").join("host.sqlite")
}

fn host_journal_path(roots: &TempRoots) -> PathBuf {
    roots.home.join(".claude").join("host.sqlite-journal")
}

fn transcript_parent(path: &Path) -> &Path {
    path.parent().expect("transcript path should have parent")
}

fn write_transcript(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write transcript");
}

fn write_host_path_sentinels(
    traversal_target: &Path,
    absolute_host_target: &Path,
    host_sqlite: &Path,
) {
    write_traversal_host_sentinel(traversal_target);
    write_absolute_host_sentinel(absolute_host_target);
    write_host_sqlite_sentinel(host_sqlite);
}

fn write_traversal_host_sentinel(path: &Path) {
    fs::write(path, traversal_host_sentinel()).expect("write traversal sentinel");
}

fn write_absolute_host_sentinel(path: &Path) {
    fs::write(path, absolute_host_sentinel()).expect("write absolute sentinel");
}

fn write_host_sqlite_sentinel(path: &Path) {
    fs::write(path, host_sqlite_sentinel()).expect("write sqlite sentinel");
}

fn traversal_host_sentinel() -> &'static [u8] {
    b"TRAVERSAL_SENTINEL"
}

fn absolute_host_sentinel() -> &'static [u8] {
    b"ABSOLUTE_HOST_SENTINEL"
}

fn host_sqlite_sentinel() -> &'static [u8] {
    b"HOST_SQLITE_SENTINEL"
}

fn write_host_state_sentinels(host_db: &Path, host_journal: &Path) {
    fs::write(host_db, host_db_sentinel()).expect("write host db sentinel");
    fs::write(host_journal, host_journal_sentinel()).expect("write host journal sentinel");
}

fn host_db_sentinel() -> &'static [u8] {
    b"host-db-sentinel"
}

fn host_journal_sentinel() -> &'static [u8] {
    b"host-journal-sentinel"
}

#[cfg(unix)]
fn create_transcript_symlink(outside: &Path, linked: &Path) {
    symlink(outside, linked).expect("create transcript symlink");
}

fn host_path_rejection_params(path: &Path, canonical: &[u8]) -> Value {
    session_replace_params("sess-host-path", path, canonical, None)
}

#[cfg(unix)]
fn symlink_rejection_params(path: &Path, canonical: &[u8]) -> Value {
    session_replace_params("sess-symlink", path, canonical, None)
}

fn invalid_canonical_params(path: &Path, original: &[u8]) -> Value {
    session_replace_params(
        "sess-invalid",
        path,
        invalid_canonical_input(),
        Some(sha256_hex(original)),
    )
}

fn preimage_mismatch_params(path: &Path, canonical: &[u8]) -> Value {
    session_replace_params("sess-conflict", path, canonical, Some(zero_sha256()))
}

fn zero_sha256() -> String {
    "0".repeat(64)
}

#[cfg(unix)]
fn write_failure_params(path: &Path, canonical: &[u8], original: &[u8]) -> Value {
    session_replace_params(
        "sess-write-failure",
        path,
        canonical,
        Some(sha256_hex(original)),
    )
}

fn noop_params(path: &Path, canonical: &[u8]) -> Value {
    session_replace_params("sess-noop", path, canonical, None)
}

fn changed_params(path: &Path, canonical: &[u8], original: &[u8]) -> Value {
    session_replace_params("sess-changed", path, canonical, Some(sha256_hex(original)))
}

fn assert_host_path_conflict(roots: &TempRoots, hostile_path: &Path, canonical: &[u8]) {
    let (code, response) = call(roots, host_path_rejection_params(hostile_path, canonical));
    assert_host_path_conflict_response(code, &response);
}

fn assert_host_path_conflict_response(code: Option<i32>, response: &Value) {
    assert_replace_error(code, response, 1, "conflict");
    assert_not_ok(response);
}

#[cfg(unix)]
fn assert_symlink_rejection(
    code: Option<i32>,
    response: &Value,
    outside_transcript: &Path,
    original: &[u8],
) {
    assert_replace_error(code, response, 1, "conflict");
    assert_not_ok(response);
    assert_transcript_bytes(outside_transcript, original);
}

fn assert_invalid_canonical_rejection(
    code: Option<i32>,
    response: &Value,
    path: &Path,
    original: &[u8],
) {
    assert_replace_error(code, response, 2, "invalid_request");
    assert_transcript_bytes(path, original);
}

fn assert_preimage_mismatch_rejection(
    code: Option<i32>,
    response: &Value,
    path: &Path,
    original: &[u8],
) {
    assert_replace_error(code, response, 1, "conflict");
    assert_transcript_bytes(path, original);
}

#[cfg(unix)]
fn assert_write_failure_rejection(
    code: Option<i32>,
    response: &Value,
    path: &Path,
    original: &[u8],
) {
    assert_replace_error(code, response, 1, "failed");
    assert_eq!(readable_transcript_bytes(path), original);
}

#[cfg(unix)]
fn assert_write_failure_retry_success(code: Option<i32>, response: &Value, path: &Path) {
    assert_success_replace_response(code, response);
    let actual_postimage = readable_postimage_bytes(path);
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

fn assert_noop_replace_response(code: Option<i32>, response: &Value, path: &Path, original: &[u8]) {
    assert_success_replace_response(code, response);
    assert!(!response["result"]["changed"].as_bool().unwrap());
    assert_eq!(response["result"]["artifacts"], json!([]));
    assert_transcript_bytes(path, original);
}

fn assert_changed_replace_outcome(
    code: Option<i32>,
    response: &Value,
    path: &Path,
    original: &[u8],
    canonical: &[u8],
    host_db: &Path,
    host_journal: &Path,
) {
    assert_success_replace_response(code, response);
    let result = &response["result"];
    assert_changed_result(result);
    let actual_native = transcript_bytes(path);
    assert_changed_native_bytes(&actual_native, original);
    assert_result_postimage_sha(result, &actual_native);
    assert_changed_native_postimage(&actual_native);
    assert_host_state_sentinels(host_db, host_journal);
    assert_changed_host_state_plan(result, canonical, &actual_native);
}

fn assert_success_replace_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceResponse",
        response,
    );
}

fn assert_replace_error(code: Option<i32>, response: &Value, expected_code: i32, category: &str) {
    assert_eq!(code, Some(expected_code));
    assert_valid(
        "session.schema.json#/$defs/SessionReplaceErrorResponse",
        response,
    );
    assert_eq!(response["error"]["category"], category);
}

fn assert_not_ok(response: &Value) {
    assert!(!response["ok"].as_bool().unwrap());
}

fn assert_host_path_sentinels(
    traversal_target: &Path,
    absolute_host_target: &Path,
    host_sqlite: &Path,
) {
    assert_file_bytes(traversal_target, traversal_host_sentinel());
    assert_file_bytes(absolute_host_target, absolute_host_sentinel());
    assert_file_bytes(host_sqlite, host_sqlite_sentinel());
}

fn assert_host_state_sentinels(host_db: &Path, host_journal: &Path) {
    assert_file_bytes(host_db, host_db_sentinel());
    assert_file_bytes(host_journal, host_journal_sentinel());
}

fn assert_transcript_bytes(path: &Path, expected: &[u8]) {
    assert_eq!(transcript_bytes(path), expected);
}

fn assert_file_bytes(path: &Path, expected: &[u8]) {
    assert_eq!(transcript_bytes(path), expected);
}

fn transcript_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

#[cfg(unix)]
fn readable_transcript_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("transcript should remain readable")
}

#[cfg(unix)]
fn readable_postimage_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).expect("postimage should be readable")
}

fn assert_changed_result(result: &Value) {
    assert!(result["changed"].as_bool().unwrap());
}

fn assert_changed_native_bytes(actual_native: &[u8], original: &[u8]) {
    assert_ne!(
        actual_native, original,
        "changed replace must publish changed native state"
    );
}

fn assert_result_postimage_sha(result: &Value, actual_native: &[u8]) {
    assert_eq!(result["postimage_sha256"], sha256_hex(actual_native));
}

fn assert_changed_native_postimage(actual_native: &[u8]) {
    let actual_text = native_transcript_text(actual_native);
    assert_native_postimage_text(&actual_text);
    let lines = native_text_lines(&actual_text);
    assert_single_native_line(&lines);
    let native_record = parse_rendered_native_record(lines[0]);
    assert_changed_native_record(&native_record);
}

fn parse_rendered_native_record(line: &str) -> Value {
    serde_json::from_str::<Value>(line).expect("native record must parse as JSON")
}

fn native_text_lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

fn assert_native_postimage_text(actual_text: &str) {
    assert!(
        actual_text.ends_with('\n'),
        "native transcript must be line-delimited JSONL"
    );
    assert!(
        !actual_text.contains("not jsonl"),
        "never leave partial transcript bytes"
    );
}

fn assert_single_native_line(lines: &[&str]) {
    assert_eq!(
        lines.len(),
        1,
        "single-turn canonical replacement must render one native record"
    );
}

fn assert_changed_native_record(native_record: &Value) {
    assert_eq!(native_record["sessionId"], "sess-changed");
    assert_eq!(native_record["uuid"], "u2");
    assert!(
        has_string(native_record, "after"),
        "native record must preserve replacement body text: {native_record}"
    );
}

fn assert_changed_host_state_plan(result: &Value, canonical: &[u8], actual_native: &[u8]) {
    let plan = &result["host_state_plan"];
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["operation"], "session.replace");
    assert_eq!(plan["session_id"], "sess-changed");
    assert_eq!(plan["provider_name"], "claude");
    assert_eq!(plan["canonical_format"], "oulipoly.canonical_transcript/v1");
    assert_eq!(plan["turn_count"], 1);
    assert_eq!(plan["records_sha256"], sha256_hex(canonical));
    assert_eq!(plan["postimage_sha256"], sha256_hex(actual_native));
    assert_eq!(plan["artifacts"][0]["kind"], "transcript");
}

#[test]
fn replace_rejects_transcript_paths_that_escape_provider_transcript_root_without_host_writes() {
    let roots = temp_roots("session-replace-host-path-confinement");
    let project_dir = prepared_transcript_dir(&roots);
    let traversal_target = host_escape_target(&roots);
    let absolute_host_target = absolute_host_transcript(&roots);
    let host_sqlite = host_sqlite_path(&roots);
    write_host_path_sentinels(&traversal_target, &absolute_host_target, &host_sqlite);

    let canonical = host_path_rejection_canonical();
    let escaped_by_traversal = escaped_host_transcript(&project_dir);
    for hostile_path in [&escaped_by_traversal, &absolute_host_target, &host_sqlite] {
        assert_host_path_conflict(&roots, hostile_path, &canonical);
    }
    assert_host_path_sentinels(&traversal_target, &absolute_host_target, &host_sqlite);
}

#[cfg(unix)]
#[test]
fn replace_rejects_symlinked_transcript_path_that_resolves_outside_root() {
    let roots = temp_roots("session-replace-symlink-confinement");
    let project_dir = prepared_transcript_dir(&roots);
    let outside_transcript = outside_transcript_path(&roots);
    let original = symlink_original_native();
    write_transcript(&outside_transcript, &original);
    let symlink_path = linked_transcript_path(&project_dir);
    create_transcript_symlink(&outside_transcript, &symlink_path);

    let canonical = symlink_rejection_canonical();
    let (code, response) = call(&roots, symlink_rejection_params(&symlink_path, &canonical));
    assert_symlink_rejection(code, &response, &outside_transcript, &original);
}

#[test]
fn replace_rejects_invalid_canonical_input_before_touching_transcript() {
    let roots = temp_roots("session-replace-invalid");
    let path = transcript_path(&roots, "invalid.jsonl");
    let original = invalid_original_native();
    write_transcript(&path, &original);

    let (code, response) = call(&roots, invalid_canonical_params(&path, &original));
    assert_invalid_canonical_rejection(code, &response, &path, &original);
}

#[test]
fn replace_preimage_mismatch_returns_conflict_and_keeps_file_byte_identical() {
    let roots = temp_roots("session-replace-conflict");
    let path = transcript_path(&roots, "conflict.jsonl");
    let original = conflict_original_native();
    write_transcript(&path, &original);
    let canonical = conflict_canonical();

    let (code, response) = call(&roots, preimage_mismatch_params(&path, &canonical));
    assert_preimage_mismatch_rejection(code, &response, &path, &original);
}

#[cfg(unix)]
#[test]
fn replace_write_failure_never_exposes_partial_transcript_and_retry_hash_matches() {
    let roots = temp_roots("session-replace-write-failure");
    let path = transcript_path(&roots, "write-failure.jsonl");
    let parent = transcript_parent(&path);
    let original = write_failure_original_native();
    write_transcript(&path, &original);
    let canonical = write_failure_canonical();
    let params = write_failure_params(&path, &canonical, &original);

    set_dir_mode(parent, 0o500);
    let (code, response) = call(&roots, params.clone());
    set_dir_mode(parent, 0o700);

    assert_write_failure_rejection(code, &response, &path, &original);

    let (code, response) = call(&roots, params);
    assert_write_failure_retry_success(code, &response, &path);
}

#[test]
fn replace_absent_preimage_allowed_noop_unchanged_reports_changed_false() {
    let roots = temp_roots("session-replace-noop");
    let path = transcript_path(&roots, "noop.jsonl");
    let original = noop_original_native();
    write_transcript(&path, &original);
    let canonical = noop_canonical();

    let (code, response) = call(&roots, noop_params(&path, &canonical));
    assert_noop_replace_response(code, &response, &path, &original);
}

#[test]
fn replace_changed_uses_atomic_native_rendering_postimage_hash_and_host_state_plan_without_host_writes(
) {
    let roots = temp_roots("session-replace-changed");
    let path = transcript_path(&roots, "changed.jsonl");
    let original = changed_original_native();
    write_transcript(&path, &original);
    let host_db = host_db_path(&roots);
    let host_journal = host_journal_path(&roots);
    write_host_state_sentinels(&host_db, &host_journal);

    let canonical = changed_canonical();
    let (code, response) = call(&roots, changed_params(&path, &canonical, &original));
    assert_changed_replace_outcome(
        code,
        &response,
        &path,
        &original,
        &canonical,
        &host_db,
        &host_journal,
    );
}
