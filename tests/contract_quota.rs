// declared_role: orchestration, mapper, accessor, validator, formatter
// intrinsic_surface_declarations:
//   - component: tests/contract_quota.rs
//     role: intrinsic-surface
//     Domain: contract_quota_proof_surface
//     Owns:
//       - quota contract scenarios
//       - support harness dependencies for quota script/invoke/schema proof

mod support;

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::thread;
use support::fixtures::{envelope, host_context, path_string, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;
use support::scripts::{
    install_probe_script, install_probe_script_with_marker, install_refresh_script,
    write_executable,
};

const WEEKLY_RESET_MS: u64 = 1_780_576_496_000;
const BURST_RESET_MS: u64 = 1_780_552_800_000;

fn quota_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn invoke_quota(subcommand: &str, roots: &TempRoots, context: Value) -> Value {
    let output = invoke(subcommand, &quota_request(roots, quota_params(context)));
    assert_success_invocation(&output);
    stdout_json(&output)
}

fn quota_params(context: Value) -> Value {
    json!({
        "settings_id": "claude-primary",
        "model_name": "claude-sonnet",
        "context": context
    })
}

fn assert_success_invocation(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(0));
    assert!(output.stderr.is_empty());
}

fn stdout_json(output: &support::invoke::Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn refresh_lock_path(roots: &TempRoots) -> std::path::PathBuf {
    roots.data_root.join("quota-refresh-claude-primary.lock")
}

fn invoke_refresh_auth(roots: &TempRoots, script: &Path) -> support::invoke::Invocation {
    invoke("quota.refresh_auth", &refresh_auth_request(roots, script))
}

fn refresh_auth_request(roots: &TempRoots, script: &Path) -> Value {
    quota_request(roots, refresh_auth_params(script))
}

fn refresh_auth_params(script: &Path) -> Value {
    json!({
        "settings_id": "claude-primary",
        "force": true,
        "context": { "auth_refresh_command": path_string(script) }
    })
}

fn assert_refresh_response(output: support::invoke::Invocation, available: bool) -> Value {
    assert_success_invocation(&output);
    let response = stdout_json(&output);
    assert_refresh_response_value(&response, available);
    response
}

fn assert_refresh_response_value(response: &Value, available: bool) {
    assert_valid(
        "quota.schema.json#/$defs/QuotaRefreshAuthResponse",
        response,
    );
    assert_eq!(response["result"]["available"], available);
}

fn quota_context(script: &Path) -> Value {
    json!({ "quota_script": path_string(script) })
}

fn quota_context_with_cache(script: &Path) -> Value {
    json!({
        "quota_script": path_string(script),
        "quota_cache": {
            "checked_at_unix_ms": 1780000000000u64,
            "windows": [{
                "name": "weekly",
                "remaining_ratio": 0.75,
                "resets_at_unix_ms": WEEKLY_RESET_MS
            }]
        }
    })
}

fn assert_probe_window(window: &Value, name: &str, remaining_ratio: f64, reset_ms: u64) {
    assert_eq!(window["name"], name);
    let actual_ratio = window["remaining_ratio"].as_f64().expect("remaining ratio");
    assert!(
        (actual_ratio - remaining_ratio).abs() < 0.000001,
        "remaining ratio {actual_ratio}"
    );
    assert_eq!(window["resets_at_unix_ms"], reset_ms);
}

fn assert_error_response(schema: &str, value: &Value) {
    assert_valid(schema, value);
    assert!(!value["ok"].as_bool().unwrap());
}

fn assert_error_invocation(
    output: support::invoke::Invocation,
    schema: &str,
    category: &str,
) -> Value {
    assert_error_status(&output);
    let response = stdout_json(&output);
    assert_error_category(schema, &response, category);
    response
}

fn assert_error_status(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
}

fn assert_error_category(schema: &str, response: &Value, category: &str) {
    assert_error_response(schema, response);
    assert_eq!(response["error"]["category"], category);
}

fn populated_windows_fixture() -> &'static str {
    r#"{
  "windows": [
    { "name": "weekly", "used_percent": 25.0, "resets_at": "2026-06-04T12:34:56Z" },
    { "name": "5h-burst", "used_percent": 80.0, "resets_at_unix_ms": 1780552800000 }
  ]
}"#
}

#[test]
fn quota_source_reports_source_freshness_and_does_not_probe_fresh_cache() {
    let roots = temp_roots("quota-source");
    let script = roots.root.join("anthropic-usage.sh");
    let marker = roots.root.join("probe-marker.txt");
    install_probe_script_with_marker(
        &script,
        populated_windows_fixture(),
        "source must not probe",
        &marker,
    );

    let response = invoke_quota("quota.source", &roots, quota_context_with_cache(&script));
    assert_quota_source_fresh_response(&response, &marker);
}

fn assert_quota_source_fresh_response(response: &Value, marker: &Path) {
    assert_valid("quota.schema.json#/$defs/QuotaSourceResponse", response);
    assert!(response["result"]["has_source"].as_bool().unwrap());
    assert!(!response["result"]["freshness"]
        .as_str()
        .expect("freshness must be a string")
        .is_empty());
    assert!(response["result"]["source_id"]
        .as_str()
        .unwrap()
        .contains("claude-primary"));
    assert!(
        !marker.exists(),
        "quota.source must not execute quota_script when cache is fresh"
    );
}

#[test]
fn quota_source_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("quota-source-malformed-request");
    let output = invoke("quota.source", &quota_request(&roots, json!({})));

    assert_error_invocation(
        output,
        "quota.schema.json#/$defs/QuotaSourceErrorResponse",
        "invalid_request",
    );
}

#[test]
fn quota_probe_parses_windows_and_legacy_flat_shapes() {
    let roots = temp_roots("quota-probe-windows");
    let script = roots.root.join("anthropic-usage.sh");
    install_probe_script(
        &script,
        populated_windows_fixture(),
        "diagnostic stderr is ignored",
    );

    let response = invoke_quota("quota.probe", &roots, quota_context(&script));
    assert_populated_probe_response(&response);

    let legacy_roots = temp_roots("quota-probe-legacy");
    let legacy_script = legacy_roots.root.join("anthropic-usage.sh");
    install_probe_script(
        &legacy_script,
        r#"{ "window": "weekly", "used_percent": 40.0, "reset_timestamp": "2026-06-04T12:34:56Z" }"#,
        "",
    );
    let legacy_response = invoke_quota("quota.probe", &legacy_roots, quota_context(&legacy_script));
    assert_legacy_probe_response(&legacy_response);
}

fn assert_populated_probe_response(response: &Value) {
    assert_valid("quota.schema.json#/$defs/QuotaProbeResponse", response);
    let windows = response["result"]["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 2);
    assert_probe_window(&windows[0], "weekly", 0.75, WEEKLY_RESET_MS);
    assert_probe_window(&windows[1], "5h-burst", 0.20, BURST_RESET_MS);
    assert!(response["result"]["available"].as_bool().unwrap());
}

fn assert_legacy_probe_response(legacy_response: &Value) {
    assert_valid(
        "quota.schema.json#/$defs/QuotaProbeResponse",
        legacy_response,
    );
    let legacy_windows = legacy_response["result"]["windows"].as_array().unwrap();
    assert_eq!(legacy_windows.len(), 1);
    assert_probe_window(&legacy_windows[0], "weekly", 0.60, WEEKLY_RESET_MS);
}

#[test]
fn quota_probe_rejects_out_of_range_malformed_and_empty_after_prior() {
    let roots = temp_roots("quota-probe-reject");
    let script = roots.root.join("bad-percent.sh");
    install_probe_script(
        &script,
        r#"{ "windows": [{ "name": "weekly", "used_percent": 101.0, "resets_at": "2026-06-04T12:34:56Z" }] }"#,
        "",
    );
    assert_probe_error_output(invoke_probe_request(&roots, probe_request_params(&script)));

    let malformed_roots = temp_roots("quota-probe-malformed");
    let malformed_script = malformed_roots.root.join("malformed.sh");
    install_probe_script(&malformed_script, "not json", "");
    assert_probe_error_output(invoke_probe_request(
        &malformed_roots,
        probe_request_params(&malformed_script),
    ));

    let empty_roots = temp_roots("quota-probe-empty-prior");
    let empty_script = empty_roots.root.join("empty.sh");
    install_probe_script(&empty_script, r#"{ "windows": [] }"#, "");
    let response = assert_probe_error_output(invoke_probe_request(
        &empty_roots,
        empty_after_prior_probe_params(&empty_script),
    ));
    assert_empty_after_prior_error(&response);
}

fn probe_request_params(script: &Path) -> Value {
    json!({ "settings_id": "claude-primary", "context": quota_context(script) })
}

fn empty_after_prior_probe_params(script: &Path) -> Value {
    json!({
        "settings_id": "claude-primary",
        "context": {
            "quota_script": path_string(script),
            "prior_windows": [{ "name": "weekly", "remaining_ratio": 0.5, "resets_at_unix_ms": WEEKLY_RESET_MS }]
        }
    })
}

fn invoke_probe_request(roots: &TempRoots, params: Value) -> support::invoke::Invocation {
    invoke("quota.probe", &quota_request(roots, params))
}

fn assert_probe_error_output(output: support::invoke::Invocation) -> Value {
    assert_ne!(output.code, Some(0));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(
        "quota.schema.json#/$defs/QuotaProbeErrorResponse",
        &response,
    );
    response
}

fn assert_empty_after_prior_error(response: &Value) {
    assert_eq!(
        response["error"]["code"],
        "quota_probe_empty_after_prior_data"
    );
    assert!(response["error"]["retryable"].as_bool().unwrap());
}

#[test]
fn quota_refresh_auth_is_provider_owned_and_single_flight_under_contention() {
    let roots = temp_roots("quota-refresh-auth");
    let script = roots.root.join("refresh.sh");
    let marker = roots.root.join("refresh-marker.txt");
    install_refresh_script(&script, &marker);

    let mut handles = Vec::new();
    for _ in 0..4 {
        let request = refresh_auth_request(&roots, &script);
        handles.push(thread::spawn(move || {
            invoke("quota.refresh_auth", &request)
        }));
    }

    assert_refresh_contention_outputs(join_refresh_outputs(handles), &marker);
}

fn join_refresh_outputs(
    handles: Vec<std::thread::JoinHandle<support::invoke::Invocation>>,
) -> Vec<support::invoke::Invocation> {
    handles
        .into_iter()
        .map(|handle| handle.join().expect("refresh thread joins"))
        .collect()
}

fn assert_refresh_contention_outputs(outputs: Vec<support::invoke::Invocation>, marker: &Path) {
    for output in outputs {
        assert_refresh_response(output, true);
    }
    assert_marker_count(
        marker,
        1,
        "concurrent refresh_auth must execute one marker command",
    );
}

#[test]
fn quota_refresh_auth_contention_preserves_failed_nonzero_outcome() {
    let roots = Arc::new(temp_roots("quota-refresh-auth-nonzero"));
    let script = roots.root.join("refresh-nonzero.sh");
    let marker = roots.root.join("refresh-nonzero-marker.txt");
    write_executable(&script, &nonzero_refresh_script(&marker));
    let barrier = Arc::new(Barrier::new(5));
    let mut handles = Vec::new();

    for _ in 0..4 {
        let roots = Arc::clone(&roots);
        let script = script.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            invoke_refresh_auth(&roots, &script)
        }));
    }
    barrier.wait();

    assert_unavailable_refresh_outputs(join_refresh_outputs(handles));
    assert_refresh_marker_and_lock(&roots, &marker);
}

fn nonzero_refresh_script(marker: &Path) -> String {
    format!(
        "#!/bin/sh\nprintf 'run\\n' >> '{}'\nsleep 1\nexit 7\n",
        marker.display()
    )
}

#[test]
fn quota_refresh_auth_timeout_preserves_unavailable_outcome_for_waiters() {
    let roots = Arc::new(temp_roots("quota-refresh-auth-timeout"));
    let script = roots.root.join("refresh-timeout.sh");
    let marker = roots.root.join("refresh-timeout-marker.txt");
    write_executable(&script, &timeout_refresh_script(&marker));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();

    for _ in 0..2 {
        let roots = Arc::clone(&roots);
        let script = script.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            invoke_refresh_auth(&roots, &script)
        }));
    }
    barrier.wait();

    assert_unavailable_refresh_outputs(join_refresh_outputs(handles));
    assert_refresh_marker_and_lock(&roots, &marker);
}

fn timeout_refresh_script(marker: &Path) -> String {
    format!(
        "#!/bin/sh\nprintf 'run\\n' >> '{}'\nexec sleep 20\n",
        marker.display()
    )
}

fn assert_unavailable_refresh_outputs(outputs: Vec<support::invoke::Invocation>) {
    for output in outputs {
        assert_refresh_response(output, false);
    }
}

fn assert_refresh_marker_and_lock(roots: &TempRoots, marker: &Path) {
    assert_marker_count(marker, 1, "refresh marker command count");
    assert!(!refresh_lock_path(roots).exists());
}

fn assert_marker_count(marker: &Path, expected: usize, message: &str) {
    let marker_text = fs::read_to_string(marker).expect("refresh marker");
    assert_eq!(marker_text.lines().count(), expected, "{message}");
}

#[test]
fn quota_refresh_auth_spawn_error_removes_lock_and_persists_unavailable_outcome() {
    let roots = temp_roots("quota-refresh-auth-spawn-error");
    let missing_script = roots.root.join("missing-refresh-command.sh");

    let output = invoke_refresh_auth(&roots, &missing_script);
    assert_spawn_error_refresh(&roots, output);

    assert_refresh_response(invoke_refresh_auth(&roots, &missing_script), false);
}

fn assert_spawn_error_refresh(roots: &TempRoots, output: support::invoke::Invocation) {
    assert_eq!(output.code, Some(4));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_error_response(
        "quota.schema.json#/$defs/QuotaRefreshAuthErrorResponse",
        &response,
    );
    assert_eq!(response["error"]["category"], "unavailable");
    assert!(!refresh_lock_path(roots).exists());
}

#[test]
fn quota_refresh_auth_recovers_from_stale_lock_before_running_owner() {
    let roots = temp_roots("quota-refresh-auth-stale-lock");
    let script = roots.root.join("refresh-stale-lock.sh");
    let marker = roots.root.join("refresh-stale-lock-marker.txt");
    install_refresh_script(&script, &marker);
    fs::write(refresh_lock_path(&roots), b"0").expect("write stale lock");

    assert_refresh_response(invoke_refresh_auth(&roots, &script), true);
    assert_refresh_marker_and_lock(&roots, &marker);
}

#[test]
fn quota_refresh_auth_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("quota-refresh-auth-malformed-request");
    let output = invoke(
        "quota.refresh_auth",
        &quota_request(&roots, json!({ "force": true })),
    );

    assert_error_invocation(
        output,
        "quota.schema.json#/$defs/QuotaRefreshAuthErrorResponse",
        "invalid_request",
    );
}
