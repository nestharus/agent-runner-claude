// declared_role: orchestration, mapper, formatter, validator, accessor

mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json, Invocation};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let output = invoke_session_locate(roots, params);
    assert_empty_stderr(&output);
    (output.code, parse_one_stdout_json(&output))
}

fn invoke_session_locate(roots: &TempRoots, params: Value) -> Invocation {
    let request = contract_request(roots, params);
    invoke("session.locate_transcript", &request)
}

fn contract_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn assert_empty_stderr(output: &Invocation) {
    assert!(output.stderr.is_empty());
}

fn claude_project_dir_path(roots: &TempRoots, name: &str) -> PathBuf {
    roots.home.join(".claude").join("projects").join(name)
}

fn child_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

fn create_dir(path: &Path) {
    fs::create_dir_all(path).expect("create directory");
}

fn ensure_claude_project_dir(roots: &TempRoots, name: &str) -> PathBuf {
    let dir = claude_project_dir_path(roots, name);
    fs::create_dir_all(&dir).expect("create claude project dir");
    dir
}

fn write_transcript(dir: &Path, filename: &str, session_id: &str) -> PathBuf {
    let path = child_path(dir, filename);
    let text = transcript_text(session_id);
    write_text_file(&path, &text);
    path
}

fn transcript_text(session_id: &str) -> String {
    let line = json!({
        "sessionId": session_id,
        "uuid": format!("{session_id}-u1"),
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "user",
        "message": { "role": "user", "content": "locate me" }
    });
    format!("{line}\n")
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn write_text_file(path: &Path, text: &str) {
    fs::write(path, text).expect("write claude transcript");
}

fn assert_session_error(value: &Value, category: &str) {
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptErrorResponse",
        value,
    );
    assert!(!value["ok"].as_bool().unwrap());
    assert_eq!(value["error"]["category"], category);
}

fn locate_request(session_id: &str) -> Value {
    json!({ "settings_id": "claude-primary", "session_id": session_id })
}

fn bounded_locate_request(session_id: &str) -> Value {
    json!({
        "settings_id": "claude-primary",
        "session_id": session_id,
        "scan": { "max_depth": 2, "max_entries": 32 }
    })
}

fn located_transcript_fixture(roots: &TempRoots) -> PathBuf {
    let project_dir = ensure_claude_project_dir(roots, "-tmp-work");
    write_transcript(&project_dir, "chat.jsonl", "sess-located")
}

fn missing_session_fixture(roots: &TempRoots) {
    let project_dir = ensure_claude_project_dir(roots, "-tmp-work");
    write_transcript(&project_dir, "other.jsonl", "other-session");
}

fn ambiguous_session_fixture(roots: &TempRoots) {
    let first_dir = ensure_claude_project_dir(roots, "-tmp-a");
    let second_dir = ensure_claude_project_dir(roots, "-tmp-b");
    write_transcript(&first_dir, "a.jsonl", "dup-session");
    write_transcript(&second_dir, "b.jsonl", "dup-session");
}

fn deep_scan_dir(project_dir: &Path) -> PathBuf {
    project_dir
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("e")
        .join("f")
}

fn bounded_scan_fixture(roots: &TempRoots) {
    let project_dir = ensure_claude_project_dir(roots, "-tmp-work");
    let too_deep = deep_scan_dir(&project_dir);
    create_dir(&too_deep);
    write_transcript(&too_deep, "deep.jsonl", "deep-session");
}

#[cfg(unix)]
fn outside_transcripts_dir(roots: &TempRoots) -> PathBuf {
    roots.root.join("outside-transcripts")
}

#[cfg(unix)]
fn outside_transcript_dir(roots: &TempRoots) -> PathBuf {
    roots.root.join("outside-transcript-dir")
}

#[cfg(unix)]
fn linked_file_path(project_dir: &Path) -> PathBuf {
    child_path(project_dir, "linked.jsonl")
}

#[cfg(unix)]
fn linked_dir_path(project_dir: &Path) -> PathBuf {
    child_path(project_dir, "linked-dir")
}

#[cfg(unix)]
fn outside_transcript_file_fixture(roots: &TempRoots) -> PathBuf {
    let outside_dir = outside_transcripts_dir(roots);
    create_dir(&outside_dir);
    write_transcript(&outside_dir, "outside.jsonl", "sess-outside-link")
}

#[cfg(unix)]
fn file_symlink_fixture(roots: &TempRoots) {
    let project_dir = ensure_claude_project_dir(roots, "-tmp-work");
    let outside_transcript = outside_transcript_file_fixture(roots);
    symlink(&outside_transcript, linked_file_path(&project_dir))
        .expect("create outside transcript symlink");
}

#[cfg(unix)]
fn directory_symlink_fixture(roots: &TempRoots) {
    let project_dir = ensure_claude_project_dir(roots, "-tmp-work");
    let outside_dir = outside_transcript_dir(roots);
    create_dir(&outside_dir);
    write_transcript(&outside_dir, "outside.jsonl", "sess-outside-dir-link");
    symlink(&outside_dir, linked_dir_path(&project_dir)).expect("create outside dir symlink");
}

fn assert_located_response(code: Option<i32>, response: &Value, expected_path: &str) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        response,
    );
    let result = &response["result"];
    assert!(result["located"].as_bool().unwrap());
    assert_eq!(result["path"], expected_path);
    assert!(Path::new(result["path"].as_str().unwrap()).is_absolute());
    assert_eq!(result["format_id"], "claude_code");
    assert_eq!(result["source_id"], "sess-located");
    assert!(result["require_existing_observed"].as_bool().unwrap());
}

fn assert_bounded_unlocated_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        response,
    );
    assert!(!response["result"]["located"].as_bool().unwrap());
}

fn assert_unlocated_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        response,
    );
    let result = &response["result"];
    assert!(!result["located"].as_bool().unwrap());
    assert!(result.get("path").is_none());
    assert!(result.get("format_id").is_none());
    assert!(result.get("source_id").is_none());
}

fn assert_session_error_response(code: Option<i32>, response: &Value, category: &str) {
    assert_eq!(code, Some(1));
    assert_session_error(response, category);
}

fn assert_failed_error_without_result(code: Option<i32>, response: &Value) {
    assert_session_error_response(code, response, "failed");
    assert!(response.get("result").is_none());
}

#[test]
fn locate_finds_existing_claude_jsonl_with_absolute_path_and_source_metadata() {
    let roots = temp_roots("session-locate-found");
    let expected_path = located_transcript_fixture(&roots);
    let expected_path = path_text(&expected_path);

    let (code, response) = call(&roots, locate_request("sess-located"));

    assert_located_response(code, &response, &expected_path);
}

#[test]
fn locate_missing_session_is_schema_valid_unlocated_result() {
    let roots = temp_roots("session-locate-missing");
    missing_session_fixture(&roots);

    let (code, response) = call(&roots, locate_request("missing-session"));

    assert_unlocated_response(code, &response);
}

#[test]
fn locate_ambiguous_session_id_returns_conflict_without_guessing() {
    let roots = temp_roots("session-locate-ambiguous");
    ambiguous_session_fixture(&roots);

    let (code, response) = call(&roots, locate_request("dup-session"));

    assert_session_error_response(code, &response, "conflict");
}

#[test]
fn locate_uses_bounded_scan_and_does_not_descend_unbounded_native_trees() {
    let roots = temp_roots("session-locate-bounded");
    bounded_scan_fixture(&roots);

    let (code, response) = call(&roots, bounded_locate_request("deep-session"));

    assert_bounded_unlocated_response(code, &response);
}

#[cfg(unix)]
#[test]
fn locate_rejects_scan_discovered_transcript_symlink_that_resolves_outside_root() {
    let roots = temp_roots("session-locate-symlink-file-confinement");
    file_symlink_fixture(&roots);

    let (code, response) = call(&roots, locate_request("sess-outside-link"));

    assert_failed_error_without_result(code, &response);
}

#[cfg(unix)]
#[test]
fn locate_rejects_scan_discovered_directory_symlink_that_resolves_outside_root() {
    let roots = temp_roots("session-locate-symlink-dir-confinement");
    directory_symlink_fixture(&roots);

    let (code, response) = call(&roots, locate_request("sess-outside-dir-link"));

    assert_failed_error_without_result(code, &response);
}
