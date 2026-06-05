mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke("session.locate_transcript", &request);
    assert!(output.stderr.is_empty());
    (output.code, parse_one_stdout_json(&output))
}

fn claude_project_dir(roots: &TempRoots, name: &str) -> PathBuf {
    let dir = roots.home.join(".claude").join("projects").join(name);
    fs::create_dir_all(&dir).expect("create claude project dir");
    dir
}

fn write_transcript(dir: &Path, filename: &str, session_id: &str) -> PathBuf {
    let path = dir.join(filename);
    let line = json!({
        "sessionId": session_id,
        "uuid": format!("{session_id}-u1"),
        "timestamp": "2026-06-04T00:00:00.000Z",
        "type": "user",
        "message": { "role": "user", "content": "locate me" }
    });
    fs::write(&path, format!("{line}\n")).expect("write claude transcript");
    path
}

fn assert_session_error(value: &Value, category: &str) {
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptErrorResponse",
        value,
    );
    assert!(!value["ok"].as_bool().unwrap());
    assert_eq!(value["error"]["category"], category);
}

#[test]
fn locate_finds_existing_claude_jsonl_with_absolute_path_and_source_metadata() {
    let roots = temp_roots("session-locate-found");
    let expected_path = write_transcript(
        &claude_project_dir(&roots, "-tmp-work"),
        "chat.jsonl",
        "sess-located",
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-located" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        &response,
    );
    let result = &response["result"];
    assert!(result["located"].as_bool().unwrap());
    assert_eq!(result["path"], expected_path.display().to_string());
    assert!(Path::new(result["path"].as_str().unwrap()).is_absolute());
    assert_eq!(result["format_id"], "claude_code");
    assert_eq!(result["source_id"], "sess-located");
    assert!(result["require_existing_observed"].as_bool().unwrap());
}

#[test]
fn locate_missing_session_is_schema_valid_unlocated_result() {
    let roots = temp_roots("session-locate-missing");
    write_transcript(
        &claude_project_dir(&roots, "-tmp-work"),
        "other.jsonl",
        "other-session",
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "missing-session" }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        &response,
    );
    let result = &response["result"];
    assert!(!result["located"].as_bool().unwrap());
    assert!(result.get("path").is_none());
    assert!(result.get("format_id").is_none());
    assert!(result.get("source_id").is_none());
}

#[test]
fn locate_ambiguous_session_id_returns_conflict_without_guessing() {
    let roots = temp_roots("session-locate-ambiguous");
    write_transcript(
        &claude_project_dir(&roots, "-tmp-a"),
        "a.jsonl",
        "dup-session",
    );
    write_transcript(
        &claude_project_dir(&roots, "-tmp-b"),
        "b.jsonl",
        "dup-session",
    );

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "dup-session" }),
    );

    assert_eq!(code, Some(2));
    assert_session_error(&response, "conflict");
}

#[test]
fn locate_uses_bounded_scan_and_does_not_descend_unbounded_native_trees() {
    let roots = temp_roots("session-locate-bounded");
    let too_deep = claude_project_dir(&roots, "-tmp-work")
        .join("a")
        .join("b")
        .join("c")
        .join("d")
        .join("e")
        .join("f");
    fs::create_dir_all(&too_deep).expect("create deep tree");
    write_transcript(&too_deep, "deep.jsonl", "deep-session");

    let (code, response) = call(
        &roots,
        json!({
            "settings_id": "claude-primary",
            "session_id": "deep-session",
            "scan": { "max_depth": 2, "max_entries": 32 }
        }),
    );

    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionLocateTranscriptResponse",
        &response,
    );
    assert!(!response["result"]["located"].as_bool().unwrap());
}

#[cfg(unix)]
#[test]
fn locate_rejects_scan_discovered_transcript_symlink_that_resolves_outside_root() {
    let roots = temp_roots("session-locate-symlink-file-confinement");
    let project_dir = claude_project_dir(&roots, "-tmp-work");
    let outside_dir = roots.root.join("outside-transcripts");
    fs::create_dir_all(&outside_dir).expect("create outside transcript dir");
    let outside_transcript = write_transcript(&outside_dir, "outside.jsonl", "sess-outside-link");
    symlink(&outside_transcript, project_dir.join("linked.jsonl"))
        .expect("create outside transcript symlink");

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-outside-link" }),
    );

    assert_eq!(code, Some(1));
    assert_session_error(&response, "failed");
    assert!(response.get("result").is_none());
}

#[cfg(unix)]
#[test]
fn locate_rejects_scan_discovered_directory_symlink_that_resolves_outside_root() {
    let roots = temp_roots("session-locate-symlink-dir-confinement");
    let project_dir = claude_project_dir(&roots, "-tmp-work");
    let outside_dir = roots.root.join("outside-transcript-dir");
    fs::create_dir_all(&outside_dir).expect("create outside transcript dir");
    write_transcript(&outside_dir, "outside.jsonl", "sess-outside-dir-link");
    symlink(&outside_dir, project_dir.join("linked-dir")).expect("create outside dir symlink");

    let (code, response) = call(
        &roots,
        json!({ "settings_id": "claude-primary", "session_id": "sess-outside-dir-link" }),
    );

    assert_eq!(code, Some(1));
    assert_session_error(&response, "failed");
    assert!(response.get("result").is_none());
}
