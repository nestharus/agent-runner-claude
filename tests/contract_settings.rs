mod support;

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> Value {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert!(output.stderr.is_empty());
    parse_one_stdout_json(&output)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert_eq!(output.code, Some(2));
    assert!(output.stderr.is_empty());
    let response = parse_one_stdout_json(&output);
    assert_valid(schema, &response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "invalid_request");
    response
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

#[test]
fn settings_crud_returns_schema_valid_redacted_records_and_opaque_versions() {
    let roots = temp_roots("settings-crud");

    let listed = call(&roots, "settings.list", json!({}));
    assert_valid("settings.schema.json#/$defs/SettingsListResponse", &listed);

    let created = call(
        &roots,
        "settings.create",
        json!({
            "display_name": "Claude Primary",
            "values": {
                "command": "claude",
                "api_key": "sk-secret",
                "auth_token": "token-secret",
                "tool_restrictions": { "kind": "claude", "claude": { "allowed_tools": ["Read"] } }
            }
        }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsCreateResponse",
        &created,
    );
    assert_no_secret_strings(&created["result"]["record"]);
    let record = &created["result"]["record"];
    let id = record["id"].as_str().unwrap();
    let v1 = record["version"].as_str().unwrap();
    assert_opaque_version(v1);

    let got = call(&roots, "settings.get", json!({ "id": id }));
    assert_valid("settings.schema.json#/$defs/SettingsGetResponse", &got);
    assert_no_secret_strings(&got["result"]["record"]);

    let updated = call(
        &roots,
        "settings.update",
        json!({
            "id": id,
            "version": v1,
            "values": { "command": "claude --model sonnet", "api_key": "sk-secret" }
        }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateResponse",
        &updated,
    );
    assert_no_secret_strings(&updated["result"]["record"]);
    let v2 = updated["result"]["record"]["version"].as_str().unwrap();
    assert_ne!(v1, v2);
    assert_opaque_version(v2);

    let deleted = call(
        &roots,
        "settings.delete",
        json!({ "id": id, "version": v2 }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsDeleteResponse",
        &deleted,
    );
    assert!(deleted["result"]["deleted"].as_bool().unwrap());
}

#[test]
fn settings_stale_version_returns_conflict_error() {
    let roots = temp_roots("settings-conflict");
    let created = call(
        &roots,
        "settings.create",
        json!({ "display_name": "conflict", "values": { "command": "claude" } }),
    );
    let id = created["result"]["record"]["id"].as_str().unwrap();
    let stale = created["result"]["record"]["version"].as_str().unwrap();
    let updated = call(
        &roots,
        "settings.update",
        json!({ "id": id, "version": stale, "values": { "command": "claude --print" } }),
    );
    let current = updated["result"]["record"]["version"].as_str().unwrap();
    assert_ne!(stale, current);

    let response = call(
        &roots,
        "settings.update",
        json!({ "id": id, "version": stale, "values": { "command": "claude --bad-stale" } }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateErrorResponse",
        &response,
    );
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "conflict");
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
        json!({ "values": { "display_name": "dry", "command": "claude", "api_key": "sk-secret" } }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsValidateResponse",
        &validated,
    );
    assert!(validated["result"]["valid"].as_bool().unwrap());

    let migrated = call(
        &roots,
        "settings.migrate",
        json!({
            "dry_run": true,
            "legacy": {
                "providers.toml": { "claude-primary": { "command": "claude", "api_key": "sk-secret" } },
                "sessions.toml": { "claude-primary": { "turn_script": "turns" } }
            }
        }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsMigrateResponse",
        &migrated,
    );

    let listed = call(&roots, "settings.list", json!({}));
    assert_valid("settings.schema.json#/$defs/SettingsListResponse", &listed);
    assert!(listed["result"]["records"].as_array().unwrap().is_empty());
}

#[test]
fn settings_concurrent_contention_exposes_only_complete_json_and_bounded_locking() {
    let roots = Arc::new(temp_roots("settings-contention"));
    let created = call(
        &roots,
        "settings.create",
        json!({ "display_name": "shared", "values": { "command": "claude" } }),
    );
    let id = created["result"]["record"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let version = Arc::new(Mutex::new(
        created["result"]["record"]["version"]
            .as_str()
            .unwrap()
            .to_string(),
    ));
    let barrier = Arc::new(Barrier::new(7));
    let started = Instant::now();

    let mut handles = Vec::new();
    for _ in 0..4 {
        let roots = Arc::clone(&roots);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..8 {
                let listed = call(&roots, "settings.list", json!({}));
                assert_valid("settings.schema.json#/$defs/SettingsListResponse", &listed);
            }
        }));
    }
    for idx in 0..2 {
        let roots = Arc::clone(&roots);
        let barrier = Arc::clone(&barrier);
        let id = id.clone();
        let version = Arc::clone(&version);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for round in 0..4 {
                let current = version.lock().unwrap().clone();
                let response = call(
                    &roots,
                    "settings.update",
                    json!({
                        "id": id,
                        "version": current,
                        "values": { "command": format!("claude --writer-{idx}-{round}") }
                    }),
                );
                if response["ok"].as_bool().unwrap() {
                    *version.lock().unwrap() = response["result"]["record"]["version"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    assert_valid(
                        "settings.schema.json#/$defs/SettingsUpdateResponse",
                        &response,
                    );
                } else {
                    assert_valid(
                        "settings.schema.json#/$defs/SettingsUpdateErrorResponse",
                        &response,
                    );
                    assert_eq!(response["error"]["category"], "conflict");
                }
            }
        }));
    }
    barrier.wait();
    for handle in handles {
        handle.join().expect("contention thread");
    }
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "settings lock contention was unbounded"
    );
}

#[test]
fn settings_multiprocess_create_update_tokens_are_unique_and_stale_conflicts_remain() {
    let roots = Arc::new(temp_roots("settings-multiprocess-unique"));
    let barrier = Arc::new(Barrier::new(17));
    let mut handles = Vec::new();

    for idx in 0..16 {
        let roots = Arc::clone(&roots);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let response = call(
                &roots,
                "settings.create",
                json!({
                    "display_name": "same-display-name",
                    "values": { "command": format!("claude --process-{idx}") }
                }),
            );
            assert_valid(
                "settings.schema.json#/$defs/SettingsCreateResponse",
                &response,
            );
            let record = &response["result"]["record"];
            (
                record["id"].as_str().unwrap().to_string(),
                record["version"].as_str().unwrap().to_string(),
            )
        }));
    }
    barrier.wait();

    let created = handles
        .into_iter()
        .map(|handle| handle.join().expect("settings creator process thread"))
        .collect::<Vec<_>>();
    let ids = created.iter().map(|(id, _)| id).collect::<BTreeSet<_>>();
    let versions = created
        .iter()
        .map(|(_, version)| version)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), created.len(), "settings record IDs collided");
    assert_eq!(versions.len(), created.len(), "settings versions collided");

    let (id, stale_version) = &created[0];
    let updated = call(
        &roots,
        "settings.update",
        json!({
            "id": id,
            "version": stale_version,
            "values": { "command": "claude --updated" }
        }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateResponse",
        &updated,
    );
    let current_version = updated["result"]["record"]["version"].as_str().unwrap();
    assert_ne!(stale_version, current_version);

    let stale = call(
        &roots,
        "settings.update",
        json!({
            "id": id,
            "version": stale_version,
            "values": { "command": "claude --stale" }
        }),
    );
    assert_valid(
        "settings.schema.json#/$defs/SettingsUpdateErrorResponse",
        &stale,
    );
    assert_eq!(stale["error"]["category"], "conflict");
}

#[test]
fn settings_versions_do_not_expose_store_layout_or_dual_file_ownership() {
    let roots = temp_roots("settings-domain");
    let a = call(
        &roots,
        "settings.create",
        json!({ "display_name": "a", "values": { "command": "claude" } }),
    );
    let b = call(
        &roots,
        "settings.create",
        json!({ "display_name": "b", "values": { "command": "claude --print" } }),
    );
    let va = a["result"]["record"]["version"].as_str().unwrap();
    let vb = b["result"]["record"]["version"].as_str().unwrap();
    assert_ne!(va, vb);
    assert_opaque_version(va);
    assert_opaque_version(vb);
}
