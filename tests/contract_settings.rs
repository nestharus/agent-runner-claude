// declared_role: orchestration, mapper, validator, predicate, accessor, parser
// intrinsic_surface_declarations:
//   - component: tests/contract_settings.rs
//     role: intrinsic-surface
//     Domain: contract_settings_proof_surface
//     Owns:
//       - settings contract scenarios
//       - support harness dependencies for settings invoke/schema proof

mod support;

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> Value {
    let output = invoke_settings(subcommand, settings_request(roots, params));
    assert_no_stderr(&output);
    stdout_json(&output)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    let output = invoke_settings(subcommand, settings_request(roots, params));
    assert_invalid_request_invocation(&output);
    let response = stdout_json(&output);
    assert_invalid_request_response(schema, &response);
    response
}

fn settings_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn invoke_settings(subcommand: &str, request: Value) -> support::invoke::Invocation {
    invoke(subcommand, &request)
}

fn assert_no_stderr(output: &support::invoke::Invocation) {
    assert!(output.stderr.is_empty());
}

fn assert_invalid_request_invocation(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(2));
    assert_no_stderr(output);
}

fn stdout_json(output: &support::invoke::Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn assert_invalid_request_response(schema: &str, response: &Value) {
    assert_valid(schema, response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "invalid_request");
}

fn assert_no_secret_strings(value: &Value) {
    match value {
        Value::String(text) => {
            assert!(!text.contains("sk-secret"), "secret leaked: {text}");
            assert!(!text.contains("token-secret"), "secret leaked: {text}");
        }
        Value::Array(items) => items.iter().for_each(assert_no_secret_strings),
        Value::Object(map) => map.values().for_each(assert_no_secret_strings),
        _ => {}
    }
}

fn assert_opaque_version(version: &str) {
    assert!(!version.contains("providers.toml"));
    assert!(!version.contains("sessions.toml"));
    assert!(!version.contains("settings"));
    assert!(!version.contains('/'));
    assert!(!version.contains('\\'));
}

fn settings_list_params() -> Value {
    json!({})
}

fn primary_settings_create_params() -> Value {
    json!({
        "display_name": "Claude Primary",
        "values": {
            "command": "claude",
            "api_key": "sk-secret",
            "auth_token": "token-secret",
            "tool_restrictions": { "kind": "claude", "claude": { "allowed_tools": ["Read"] } }
        }
    })
}

fn settings_get_params(id: &str) -> Value {
    json!({ "id": id })
}

fn primary_settings_update_params(id: &str, version: &str) -> Value {
    json!({
        "id": id,
        "version": version,
        "values": { "command": "claude --model sonnet", "api_key": "sk-secret" }
    })
}

fn settings_delete_params(id: &str, version: &str) -> Value {
    json!({ "id": id, "version": version })
}

fn conflict_settings_create_params() -> Value {
    json!({ "display_name": "conflict", "values": { "command": "claude" } })
}

fn settings_command_update_params(id: &str, version: &str, command: &str) -> Value {
    json!({ "id": id, "version": version, "values": { "command": command } })
}

fn settings_validate_dry_run_params() -> Value {
    json!({ "values": { "display_name": "dry", "command": "claude", "api_key": "sk-secret" } })
}

fn settings_dry_run_migrate_params() -> Value {
    json!({
        "dry_run": true,
        "legacy": {
            "providers.toml": { "claude-primary": { "command": "claude", "api_key": "sk-secret" } },
            "sessions.toml": { "claude-primary": { "turn_script": "turns" } }
        }
    })
}

fn shared_settings_create_params() -> Value {
    json!({ "display_name": "shared", "values": { "command": "claude" } })
}

fn writer_settings_update_params(id: &str, version: &str, idx: usize, round: usize) -> Value {
    json!({
        "id": id,
        "version": version,
        "values": { "command": format!("claude --writer-{idx}-{round}") }
    })
}

fn process_settings_create_params(idx: usize) -> Value {
    json!({
        "display_name": "same-display-name",
        "values": { "command": format!("claude --process-{idx}") }
    })
}

fn domain_settings_create_params(display_name: &str, command: &str) -> Value {
    json!({ "display_name": display_name, "values": { "command": command } })
}

fn response_record(response: &Value) -> &Value {
    &response["result"]["record"]
}

fn response_record_id(response: &Value) -> &str {
    response_record(response)["id"].as_str().unwrap()
}

fn response_record_version(response: &Value) -> &str {
    response_record(response)["version"].as_str().unwrap()
}

fn response_ok(response: &Value) -> bool {
    response["ok"].as_bool().unwrap()
}

fn assert_settings_list_response(response: &Value) {
    assert_valid("settings.schema.json#/$defs/SettingsListResponse", response);
}

fn assert_settings_create_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsCreateResponse",
        response,
    );
}

fn assert_settings_create_response_redacted(response: &Value) {
    assert_settings_create_response(response);
    assert_no_secret_strings(response_record(response));
}

fn assert_settings_get_response_redacted(response: &Value) {
    assert_valid("settings.schema.json#/$defs/SettingsGetResponse", response);
    assert_no_secret_strings(response_record(response));
}

fn assert_settings_update_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateResponse",
        response,
    );
}

fn assert_settings_update_response_redacted(response: &Value) {
    assert_settings_update_response(response);
    assert_no_secret_strings(response_record(response));
}

fn assert_settings_delete_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsDeleteResponse",
        response,
    );
}

fn assert_settings_deleted(response: &Value) {
    assert!(response["result"]["deleted"].as_bool().unwrap());
}

fn assert_versions_differ(previous: &str, current: &str) {
    assert_ne!(previous, current);
}

fn assert_stale_version_conflict_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateErrorResponse",
        response,
    );
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "conflict");
}

fn assert_update_conflict_schema_and_category(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateErrorResponse",
        response,
    );
    assert_eq!(response["error"]["category"], "conflict");
}

fn assert_settings_validate_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsValidateResponse",
        response,
    );
}

fn assert_settings_validate_valid(response: &Value) {
    assert!(response["result"]["valid"].as_bool().unwrap());
}

fn assert_settings_migrate_response(response: &Value) {
    assert_valid(
        "settings.schema.json#/$defs/SettingsMigrateResponse",
        response,
    );
}

fn assert_settings_list_empty(response: &Value) {
    assert!(response["result"]["records"].as_array().unwrap().is_empty());
}

fn assert_settings_list_responses(responses: &[Value]) {
    for response in responses {
        assert_settings_list_response(response);
    }
}

fn settings_list_responses(roots: &TempRoots, count: usize) -> Vec<Value> {
    (0..count)
        .map(|_| call(roots, "settings.list", settings_list_params()))
        .collect()
}

fn spawn_settings_reader(
    roots: &Arc<TempRoots>,
    barrier: &Arc<Barrier>,
) -> std::thread::JoinHandle<()> {
    let roots = Arc::clone(roots);
    let barrier = Arc::clone(barrier);
    std::thread::spawn(move || settings_reader_worker(roots, barrier))
}

fn settings_reader_worker(roots: Arc<TempRoots>, barrier: Arc<Barrier>) {
    barrier.wait();
    let responses = settings_list_responses(&roots, 8);
    assert_settings_list_responses(&responses);
}

fn spawn_settings_writer(
    roots: &Arc<TempRoots>,
    barrier: &Arc<Barrier>,
    id: &str,
    version: &Arc<Mutex<String>>,
    idx: usize,
) -> std::thread::JoinHandle<()> {
    let roots = Arc::clone(roots);
    let barrier = Arc::clone(barrier);
    let id = id.to_string();
    let version = Arc::clone(version);
    std::thread::spawn(move || settings_writer_worker(roots, barrier, id, version, idx))
}

fn settings_writer_worker(
    roots: Arc<TempRoots>,
    barrier: Arc<Barrier>,
    id: String,
    version: Arc<Mutex<String>>,
    idx: usize,
) {
    barrier.wait();
    for round in 0..4 {
        settings_writer_round(&roots, &id, &version, idx, round);
    }
}

fn settings_writer_round(
    roots: &TempRoots,
    id: &str,
    version: &Mutex<String>,
    idx: usize,
    round: usize,
) {
    let current = shared_version_text(version);
    let response = settings_writer_response(roots, id, &current, idx, round);
    update_shared_version_if_success(version, &response);
    assert_writer_update_response(&response);
}

fn shared_version_text(version: &Mutex<String>) -> String {
    version.lock().unwrap().clone()
}

fn settings_writer_response(
    roots: &TempRoots,
    id: &str,
    version: &str,
    idx: usize,
    round: usize,
) -> Value {
    call(
        roots,
        "settings.update",
        writer_settings_update_params(id, version, idx, round),
    )
}

fn update_shared_version_if_success(version: &Mutex<String>, response: &Value) {
    if !response_ok(response) {
        return;
    }
    write_shared_version(version, response_record_version(response));
}

fn write_shared_version(version: &Mutex<String>, current: &str) {
    *version.lock().unwrap() = current.to_string();
}

fn assert_writer_update_response(response: &Value) {
    if response_ok(response) {
        assert_settings_update_response(response);
        return;
    }
    assert_update_conflict_schema_and_category(response);
}

fn join_settings_threads(handles: Vec<std::thread::JoinHandle<()>>) {
    for handle in handles {
        handle.join().expect("contention thread");
    }
}

fn assert_contention_bounded(started: Instant) {
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "settings lock contention was unbounded"
    );
}

fn spawn_settings_creator(
    roots: &Arc<TempRoots>,
    barrier: &Arc<Barrier>,
    idx: usize,
) -> std::thread::JoinHandle<(String, String)> {
    let roots = Arc::clone(roots);
    let barrier = Arc::clone(barrier);
    std::thread::spawn(move || settings_creator_worker(roots, barrier, idx))
}

fn settings_creator_worker(
    roots: Arc<TempRoots>,
    barrier: Arc<Barrier>,
    idx: usize,
) -> (String, String) {
    barrier.wait();
    let response = call(
        &roots,
        "settings.create",
        process_settings_create_params(idx),
    );
    assert_settings_create_response(&response);
    response_record_tokens(&response)
}

fn response_record_tokens(response: &Value) -> (String, String) {
    (
        response_record_id(response).to_string(),
        response_record_version(response).to_string(),
    )
}

fn join_settings_creators(
    handles: Vec<std::thread::JoinHandle<(String, String)>>,
) -> Vec<(String, String)> {
    handles
        .into_iter()
        .map(|handle| handle.join().expect("settings creator process thread"))
        .collect()
}

fn created_record_ids(created: &[(String, String)]) -> BTreeSet<&str> {
    created.iter().map(|(id, _)| id.as_str()).collect()
}

fn created_record_versions(created: &[(String, String)]) -> BTreeSet<&str> {
    created
        .iter()
        .map(|(_, version)| version.as_str())
        .collect()
}

fn assert_created_records_unique(
    created: &[(String, String)],
    ids: &BTreeSet<&str>,
    versions: &BTreeSet<&str>,
) {
    assert_eq!(ids.len(), created.len(), "settings record IDs collided");
    assert_eq!(versions.len(), created.len(), "settings versions collided");
}

fn first_created_record(created: &[(String, String)]) -> (&str, &str) {
    let (id, version) = &created[0];
    (id.as_str(), version.as_str())
}

fn assert_distinct_opaque_versions(first: &str, second: &str) {
    assert_versions_differ(first, second);
    assert_opaque_version(first);
    assert_opaque_version(second);
}

#[test]
fn settings_crud_returns_schema_valid_redacted_records_and_opaque_versions() {
    let roots = temp_roots("settings-crud");

    let listed = call(&roots, "settings.list", settings_list_params());
    assert_settings_list_response(&listed);

    let created = call(&roots, "settings.create", primary_settings_create_params());
    assert_settings_create_response_redacted(&created);
    let id = response_record_id(&created);
    let v1 = response_record_version(&created);
    assert_opaque_version(v1);

    let got = call(&roots, "settings.get", settings_get_params(id));
    assert_settings_get_response_redacted(&got);

    let updated = call(
        &roots,
        "settings.update",
        primary_settings_update_params(id, v1),
    );
    assert_settings_update_response_redacted(&updated);
    let v2 = response_record_version(&updated);
    assert_versions_differ(v1, v2);
    assert_opaque_version(v2);

    let deleted = call(&roots, "settings.delete", settings_delete_params(id, v2));
    assert_settings_delete_response(&deleted);
    assert_settings_deleted(&deleted);
}

#[test]
fn settings_stale_version_returns_conflict_error() {
    let roots = temp_roots("settings-conflict");
    let created = call(&roots, "settings.create", conflict_settings_create_params());
    let id = response_record_id(&created);
    let stale = response_record_version(&created);
    let updated = call(
        &roots,
        "settings.update",
        settings_command_update_params(id, stale, "claude --print"),
    );
    let current = response_record_version(&updated);
    assert_versions_differ(stale, current);

    let response = call(
        &roots,
        "settings.update",
        settings_command_update_params(id, stale, "claude --bad-stale"),
    );
    assert_stale_version_conflict_response(&response);
}

#[test]
fn settings_list_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("settings-list-malformed-request");

    call_error(
        &roots,
        "settings.list",
        json!(null),
        "settings.schema.json#/$defs/SettingsListErrorResponse",
    );
}

#[test]
fn settings_get_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("settings-get-malformed-request");

    call_error(
        &roots,
        "settings.get",
        json!({}),
        "settings.schema.json#/$defs/SettingsGetErrorResponse",
    );
}

#[test]
fn settings_create_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("settings-create-malformed-request");

    call_error(
        &roots,
        "settings.create",
        json!({}),
        "settings.schema.json#/$defs/SettingsCreateErrorResponse",
    );
}

#[test]
fn settings_delete_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("settings-delete-malformed-request");

    call_error(
        &roots,
        "settings.delete",
        json!({}),
        "settings.schema.json#/$defs/SettingsDeleteErrorResponse",
    );
}

#[test]
fn settings_validate_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("settings-validate-malformed-request");

    call_error(
        &roots,
        "settings.validate",
        json!({}),
        "settings.schema.json#/$defs/SettingsValidateErrorResponse",
    );
}

#[test]
fn settings_migrate_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("settings-migrate-malformed-request");

    call_error(
        &roots,
        "settings.migrate",
        json!({}),
        "settings.schema.json#/$defs/SettingsMigrateErrorResponse",
    );
}

#[test]
fn settings_validate_and_dry_run_migrate_never_persist() {
    let roots = temp_roots("settings-dry-run");

    let validated = call(
        &roots,
        "settings.validate",
        settings_validate_dry_run_params(),
    );
    assert_settings_validate_response(&validated);
    assert_settings_validate_valid(&validated);

    let migrated = call(
        &roots,
        "settings.migrate",
        settings_dry_run_migrate_params(),
    );
    assert_settings_migrate_response(&migrated);

    let listed = call(&roots, "settings.list", settings_list_params());
    assert_settings_list_response(&listed);
    assert_settings_list_empty(&listed);
}

#[test]
fn settings_concurrent_contention_exposes_only_complete_json_and_bounded_locking() {
    let roots = Arc::new(temp_roots("settings-contention"));
    let created = call(&roots, "settings.create", shared_settings_create_params());
    let id = response_record_id(&created).to_string();
    let version = Arc::new(Mutex::new(response_record_version(&created).to_string()));
    let barrier = Arc::new(Barrier::new(7));
    let started = Instant::now();

    let mut handles = Vec::new();
    for _ in 0..4 {
        handles.push(spawn_settings_reader(&roots, &barrier));
    }
    for idx in 0..2 {
        handles.push(spawn_settings_writer(&roots, &barrier, &id, &version, idx));
    }
    barrier.wait();
    join_settings_threads(handles);
    assert_contention_bounded(started);
}

#[test]
fn settings_multiprocess_create_update_tokens_are_unique_and_stale_conflicts_remain() {
    let roots = Arc::new(temp_roots("settings-multiprocess-unique"));
    let barrier = Arc::new(Barrier::new(17));
    let mut handles = Vec::new();

    for idx in 0..16 {
        handles.push(spawn_settings_creator(&roots, &barrier, idx));
    }
    barrier.wait();

    let created = join_settings_creators(handles);
    let ids = created_record_ids(&created);
    let versions = created_record_versions(&created);
    assert_created_records_unique(&created, &ids, &versions);

    let (id, stale_version) = first_created_record(&created);
    let updated = call(
        &roots,
        "settings.update",
        settings_command_update_params(id, stale_version, "claude --updated"),
    );
    assert_settings_update_response(&updated);
    let current_version = response_record_version(&updated);
    assert_versions_differ(stale_version, current_version);

    let stale = call(
        &roots,
        "settings.update",
        settings_command_update_params(id, stale_version, "claude --stale"),
    );
    assert_update_conflict_schema_and_category(&stale);
}

#[test]
fn settings_versions_do_not_expose_store_layout_or_dual_file_ownership() {
    let roots = temp_roots("settings-domain");
    let a = call(
        &roots,
        "settings.create",
        domain_settings_create_params("a", "claude"),
    );
    let b = call(
        &roots,
        "settings.create",
        domain_settings_create_params("b", "claude --print"),
    );
    let va = response_record_version(&a);
    let vb = response_record_version(&b);
    assert_distinct_opaque_versions(va, vb);
}
