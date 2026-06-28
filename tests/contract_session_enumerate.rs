// declared_role: orchestration, mapper, formatter, validator, accessor
// intrinsic_surface_declarations:
//   - component: tests/contract_session_enumerate.rs
//     role: intrinsic-surface
//     Domain: contract_session_enumerate_proof_surface
//     Owns:
//       - session enumerate contract scenarios
//       - synthetic Claude projects transcript fixtures under temp HOME

mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json, Invocation};
use support::schema::assert_valid;

fn call(roots: &TempRoots, params: Value) -> (Option<i32>, Value) {
    let output = invoke_session_enumerate(roots, params);
    assert_empty_stderr(&output);
    (output.code, parse_one_stdout_json(&output))
}

fn invoke_session_enumerate(roots: &TempRoots, params: Value) -> Invocation {
    let request = envelope(CONTRACT, host_context(roots), params);
    invoke("session.enumerate", &request)
}

fn assert_empty_stderr(output: &Invocation) {
    assert!(output.stderr.is_empty());
}

fn claude_project_dir(roots: &TempRoots, project: &str) -> PathBuf {
    roots.home.join(".claude").join("projects").join(project)
}

fn transcript_path(roots: &TempRoots, project: &str, file: &str) -> PathBuf {
    let dir = claude_project_dir(roots, project);
    fs::create_dir_all(&dir).expect("create claude project dir");
    dir.join(file)
}

fn write_session(
    roots: &TempRoots,
    project: &str,
    session_id: &str,
    timestamp: &str,
    cwd: Option<&Path>,
    title: Option<&str>,
) -> PathBuf {
    let path = transcript_path(roots, project, &format!("{session_id}.jsonl"));
    write_jsonl(&path, &[session_record(session_id, timestamp, cwd, title)]);
    path
}

fn write_jsonl(path: &Path, records: &[Value]) {
    let text = records
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{text}\n")).expect("write jsonl transcript");
}

fn session_record(
    session_id: &str,
    timestamp: &str,
    cwd: Option<&Path>,
    title: Option<&str>,
) -> Value {
    let mut record = json!({
        "sessionId": session_id,
        "uuid": format!("{session_id}-u1"),
        "timestamp": timestamp,
        "type": "user",
        "message": { "role": "user", "content": "hello" }
    });
    if let Some(cwd) = cwd {
        record["cwd"] = json!(cwd.display().to_string());
    }
    if let Some(title) = title {
        record["title"] = json!(title);
    }
    record
}

fn enumerate_params(extra: Value) -> Value {
    let mut params = json!({ "settings_id": "claude-primary" });
    for (key, value) in extra.as_object().expect("extra params object") {
        params[key] = value.clone();
    }
    params
}

fn result_sessions(response: &Value) -> &[Value] {
    response["result"]["sessions"].as_array().unwrap()
}

fn session_ids(response: &Value) -> Vec<String> {
    result_sessions(response)
        .iter()
        .map(|session| session["provider_session_id"].as_str().unwrap().to_string())
        .collect()
}

fn ts_ms(timestamp: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .unwrap()
        .timestamp_millis()
}

fn assert_enumerate_response(code: Option<i32>, response: &Value) {
    assert_eq!(code, Some(0));
    assert_valid(
        "session.schema.json#/$defs/SessionEnumerateResponse",
        response,
    );
}

fn warnings(response: &Value) -> Vec<String> {
    response["result"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning.as_str().unwrap().to_string())
        .collect()
}

fn assert_warning_contains(response: &Value, needle: &str) {
    assert!(
        warnings(response)
            .iter()
            .any(|warning| warning.contains(needle)),
        "expected warning containing {needle:?}, got {:?}",
        warnings(response)
    );
}

#[test]
fn enumerate_scans_jsonl_sessions_by_filename_stem_and_sorts_deterministically() {
    let roots = temp_roots("session-enumerate-normal");
    let cwd = roots.root.join("workspace-a");
    fs::create_dir_all(&cwd).expect("create cwd fixture");
    write_session(
        &roots,
        "-repo-a",
        "sess-a",
        "2026-06-05T00:00:00.000Z",
        Some(&cwd),
        Some("Alpha"),
    );
    write_session(
        &roots,
        "-home-nes-ambiguous-project-name",
        "sess-b",
        "2026-06-04T00:00:00.000Z",
        None,
        None,
    );
    let ignored = transcript_path(&roots, "-repo-a", "not-a-session.txt");
    fs::write(ignored, "not jsonl").expect("write ignored fixture");

    let (code, response) = call(
        &roots,
        enumerate_params(json!({ "include_cwd": true, "include_turn_count": true })),
    );

    assert_enumerate_response(code, &response);
    assert_eq!(response["result"]["complete"], true);
    assert!(response["result"]["next_cursor"].is_null());
    assert_eq!(warnings(&response), Vec::<String>::new());
    assert_eq!(session_ids(&response), vec!["sess-a", "sess-b"]);

    let sessions = result_sessions(&response);
    assert_eq!(sessions[0]["title"], "Alpha");
    assert_eq!(sessions[0]["cwd"], cwd.display().to_string());
    assert_eq!(
        sessions[0]["created_unix_ms"],
        ts_ms("2026-06-05T00:00:00.000Z")
    );
    assert_eq!(
        sessions[0]["updated_unix_ms"],
        ts_ms("2026-06-05T00:00:00.000Z")
    );
    assert_eq!(sessions[0]["turn_count"], 1);
    assert_eq!(sessions[0]["source"]["kind"], "claude.projects_scan");
    assert_eq!(sessions[0]["source"]["detail"], "-repo-a/sess-a.jsonl");

    assert!(sessions[1]["cwd"].is_null());
    assert!(sessions[1]["title"].is_null());
    assert_eq!(sessions[1]["turn_count"], 1);
}

#[test]
fn enumerate_deduplicates_session_ids_and_skips_malformed_jsonl_with_warnings() {
    let roots = temp_roots("session-enumerate-duplicates");
    write_session(
        &roots,
        "-repo-a",
        "dup-session",
        "2026-06-04T00:00:00.000Z",
        None,
        None,
    );
    write_session(
        &roots,
        "-repo-b",
        "dup-session",
        "2026-06-05T00:00:00.000Z",
        None,
        None,
    );
    let malformed = transcript_path(&roots, "-repo-c", "bad-session.jsonl");
    fs::write(malformed, "{not-json\n").expect("write malformed transcript");

    let (code, response) = call(&roots, enumerate_params(json!({})));

    assert_enumerate_response(code, &response);
    assert_eq!(session_ids(&response), vec!["dup-session"]);
    assert_eq!(
        result_sessions(&response)[0]["source"]["detail"],
        "-repo-b/dup-session.jsonl"
    );
    assert_warning_contains(&response, "duplicate provider_session_id dup-session");
    assert_warning_contains(&response, "bad-session.jsonl");
}

#[test]
fn enumerate_honors_since_limit_and_ignores_cursor_for_v1() {
    let roots = temp_roots("session-enumerate-limit-since");
    write_session(
        &roots,
        "-repo",
        "old-session",
        "2026-06-03T00:00:00.000Z",
        None,
        None,
    );
    write_session(
        &roots,
        "-repo",
        "mid-session",
        "2026-06-04T00:00:00.000Z",
        None,
        None,
    );
    write_session(
        &roots,
        "-repo",
        "new-session",
        "2026-06-05T00:00:00.000Z",
        None,
        None,
    );

    let (code, response) = call(
        &roots,
        enumerate_params(json!({
            "since_unix_ms": ts_ms("2026-06-04T00:00:00.000Z"),
            "limit": 1,
            "cursor": "ignored-cursor"
        })),
    );

    assert_enumerate_response(code, &response);
    assert_eq!(session_ids(&response), vec!["new-session"]);
    assert_eq!(response["result"]["complete"], true);
    assert!(response["result"]["next_cursor"].is_null());
}

#[test]
fn enumerate_reports_depth_and_entry_bounds_without_panicking() {
    let depth_roots = temp_roots("session-enumerate-depth-bound");
    let deep_dir = claude_project_dir(&depth_roots, "-repo")
        .join("a")
        .join("b")
        .join("c");
    fs::create_dir_all(&deep_dir).expect("create deep dir");
    write_jsonl(
        &deep_dir.join("deep-session.jsonl"),
        &[session_record(
            "deep-session",
            "2026-06-04T00:00:00.000Z",
            None,
            None,
        )],
    );

    let (code, response) = call(
        &depth_roots,
        enumerate_params(json!({ "scan": { "max_depth": 1, "max_entries": 32 } })),
    );
    assert_enumerate_response(code, &response);
    assert!(result_sessions(&response).is_empty());
    assert_warning_contains(&response, "max_depth=1");

    let entry_roots = temp_roots("session-enumerate-entry-bound");
    for index in 0..5 {
        write_session(
            &entry_roots,
            "-repo",
            &format!("entry-session-{index}"),
            "2026-06-04T00:00:00.000Z",
            None,
            None,
        );
    }

    let (code, response) = call(
        &entry_roots,
        enumerate_params(json!({ "scan": { "max_depth": 8, "max_entries": 2 } })),
    );
    assert_enumerate_response(code, &response);
    assert_warning_contains(&response, "max_entries=2");
}
