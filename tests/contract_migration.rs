mod support;

use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> Value {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert_eq!(output.code, Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_one_stdout_json(&output)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert_eq!(output.code, Some(2));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = parse_one_stdout_json(&output);
    assert_valid(schema, &response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], "invalid_request");
    response
}

fn call_error_category(
    roots: &TempRoots,
    subcommand: &str,
    params: Value,
    schema: &str,
    category: &str,
) -> Value {
    let request = envelope(CONTRACT, host_context(roots), params);
    let output = invoke(subcommand, &request);
    assert_eq!(output.code, Some(2));
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = parse_one_stdout_json(&output);
    assert_valid(schema, &response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], category);
    response
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).expect("write fixture");
}

fn assert_file(path: &Path, expected: &str) {
    assert_eq!(fs::read_to_string(path).expect("read fixture"), expected);
}

fn has_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| has_string(item, needle)),
        Value::Object(map) => map.values().any(|item| has_string(item, needle)),
        _ => false,
    }
}

fn assert_provider_owned_action(action: &Value) {
    assert!(
        has_string(action, "claude"),
        "migration action is not provider-owned: {action}"
    );
    assert!(
        !has_string(action, "central-state.sqlite"),
        "migration action targets host central state: {action}"
    );
}

fn assert_artifact_digest(artifact: &Value) {
    let sha = artifact["sha256"].as_str().expect("artifact sha256");
    assert_eq!(sha.len(), 64);
    assert!(sha
        .chars()
        .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
}

#[test]
fn migration_plan_returns_provider_owned_actions_backup_confirmation_warnings_and_no_mutation() {
    let roots = temp_roots("migration-plan");
    let legacy_config = roots.home.join(".claude.json");
    let provider_file = roots.home.join(".claude/settings.json");
    let central_state = roots.data_root.join("central-state.sqlite");
    fs::create_dir_all(provider_file.parent().unwrap()).unwrap();
    write(&legacy_config, "LEGACY_CONFIG_SENTINEL");
    write(&provider_file, "PROVIDER_FILE_SENTINEL");
    write(&central_state, "CENTRAL_STATE_PLAN_SENTINEL");

    let response = call(
        &roots,
        "migration.plan",
        json!({
            "from": "legacy-host-state",
            "to": "claude.settings/v1",
            "provider_root": roots.home.join(".claude").display().to_string(),
            "legacy": {
                "providers.toml": { "claude-primary": { "command": "claude" } },
                "sessions.toml": { "claude-primary": { "turn_script": "turns" } }
            },
            "central_state": central_state.display().to_string()
        }),
    );
    assert_valid(
        "migration.schema.json#/$defs/MigrationPlanResponse",
        &response,
    );
    let result = &response["result"];
    assert!(!result["actions"].as_array().unwrap().is_empty());
    for action in result["actions"].as_array().unwrap() {
        assert_provider_owned_action(action);
    }
    assert!(result["requires_backup"].as_bool().unwrap());
    assert!(
        result.get("confirmation").is_some(),
        "migration.plan must include confirmation metadata"
    );
    assert!(
        has_string(&result["warnings"], "backup")
            || !result["warnings"].as_array().unwrap().is_empty()
    );

    assert_file(&legacy_config, "LEGACY_CONFIG_SENTINEL");
    assert_file(&provider_file, "PROVIDER_FILE_SENTINEL");
    assert_file(&central_state, "CENTRAL_STATE_PLAN_SENTINEL");
}

#[test]
fn migration_plan_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("migration-plan-malformed-request");

    call_error(
        &roots,
        "migration.plan",
        json!(null),
        "migration.schema.json#/$defs/MigrationPlanErrorResponse",
    );
}

#[test]
fn migration_plan_rejects_provider_root_outside_declared_roots_even_when_name_contains_claude() {
    let roots = temp_roots("migration-plan-root-confinement");
    let hostile_root = roots.root.join("host-owned-claude-looking-root");
    fs::create_dir_all(&hostile_root).expect("create hostile root");
    let sentinel = hostile_root.join("settings.v1.json");
    write(&sentinel, "HOSTILE_ROOT_SENTINEL");

    let response = call_error_category(
        &roots,
        "migration.plan",
        json!({
            "from": "legacy-host-state",
            "to": "claude.settings/v1",
            "provider_root": hostile_root.display().to_string(),
            "legacy": {
                "providers.toml": { "claude-primary": { "command": "claude" } },
                "sessions.toml": { "claude-primary": { "turn_script": "turns" } }
            }
        }),
        "migration.schema.json#/$defs/MigrationPlanErrorResponse",
        "conflict",
    );

    assert_eq!(
        response["error"]["code"],
        "migration_provider_root_outside_provider_root"
    );
    assert_file(&sentinel, "HOSTILE_ROOT_SENTINEL");
}

#[test]
fn migration_apply_only_performs_confirmed_provider_owned_actions_and_leaves_host_central_state() {
    let roots = temp_roots("migration-apply");
    let provider_root = roots.home.join(".claude");
    fs::create_dir_all(&provider_root).unwrap();
    let central_state = roots.data_root.join("central-state.sqlite");
    write(&central_state, "CENTRAL_STATE_SENTINEL_BEFORE_MIGRATION");
    let unconfirmed_provider_file = provider_root.join("unconfirmed.json");
    write(&unconfirmed_provider_file, "UNCONFIRMED_PROVIDER_SENTINEL");

    let response = call(
        &roots,
        "migration.apply",
        json!({
            "confirmed": true,
            "confirmation": {
                "id": "confirm-provider-owned-w2c",
                "accepted": true,
                "backup_root": roots.data_root.join("migration-backup").display().to_string()
            },
            "actions": [
                {
                    "kind": "write_file",
                    "provider_owned": true,
                    "path": provider_root.join("settings.v1.json").display().to_string(),
                    "content": { "encoding": "utf8", "data": "{\"provider\":\"claude\"}" }
                },
                {
                    "kind": "write_file",
                    "provider_owned": false,
                    "path": central_state.display().to_string(),
                    "content": { "encoding": "utf8", "data": "SHOULD_NOT_BE_WRITTEN" }
                },
                {
                    "kind": "write_file",
                    "provider_owned": true,
                    "confirmed": false,
                    "path": unconfirmed_provider_file.display().to_string(),
                    "content": { "encoding": "utf8", "data": "SHOULD_NOT_OVERWRITE_UNCONFIRMED" }
                }
            ]
        }),
    );
    assert_valid(
        "migration.schema.json#/$defs/MigrationApplyResponse",
        &response,
    );
    let result = &response["result"];
    assert!(!result["applied_actions"].as_array().unwrap().is_empty());
    for action in result["applied_actions"].as_array().unwrap() {
        assert_provider_owned_action(action);
        assert!(
            !has_string(action, "unconfirmed.json"),
            "unconfirmed provider action was applied: {action}"
        );
    }
    assert!(!result["artifacts"].as_array().unwrap().is_empty());
    for artifact in result["artifacts"].as_array().unwrap() {
        assert!(artifact["path"]
            .as_str()
            .unwrap_or_default()
            .contains("claude"));
        assert_artifact_digest(artifact);
    }
    assert!(has_string(&result["outcome"], "applied") || result["outcome"].is_object());

    assert_file(&central_state, "CENTRAL_STATE_SENTINEL_BEFORE_MIGRATION");
    assert_file(&unconfirmed_provider_file, "UNCONFIRMED_PROVIDER_SENTINEL");
}

#[test]
fn migration_apply_rejects_provider_owned_claim_for_host_path_and_leaves_sentinel_unchanged() {
    let roots = temp_roots("migration-apply-host-path-confinement");
    let central_state = roots.data_root.join("central-state.sqlite");
    write(&central_state, "HOST_CENTRAL_STATE_SENTINEL");

    let response = call_error_category(
        &roots,
        "migration.apply",
        json!({
            "confirmed": true,
            "confirmation": {
                "id": "confirm-host-path-adversarial",
                "accepted": true,
                "backup_root": roots.data_root.join("migration-backup").display().to_string()
            },
            "actions": [
                {
                    "kind": "write_file",
                    "provider_owned": true,
                    "path": central_state.display().to_string(),
                    "content": { "encoding": "utf8", "data": "SHOULD_NOT_OVERWRITE_HOST_STATE" }
                }
            ]
        }),
        "migration.schema.json#/$defs/MigrationApplyErrorResponse",
        "conflict",
    );

    assert_eq!(
        response["error"]["code"],
        "migration_action_outside_provider_root"
    );
    assert_file(&central_state, "HOST_CENTRAL_STATE_SENTINEL");
}

#[cfg(unix)]
#[test]
fn migration_apply_rejects_provider_owned_path_that_resolves_outside_root() {
    let roots = temp_roots("migration-apply-symlink-path-confinement");
    let provider_root = roots.home.join(".claude");
    let outside_root = roots.root.join("outside-provider");
    fs::create_dir_all(&provider_root).expect("create provider root");
    fs::create_dir_all(&outside_root).expect("create outside root");
    let outside_file = outside_root.join("settings.v1.json");
    write(&outside_file, "OUTSIDE_PROVIDER_SENTINEL");
    symlink(&outside_root, provider_root.join("linked-outside")).expect("create outside symlink");

    let response = call_error_category(
        &roots,
        "migration.apply",
        json!({
            "confirmed": true,
            "confirmation": {
                "id": "confirm-symlink-path-adversarial",
                "accepted": true,
                "backup_root": roots.data_root.join("migration-backup").display().to_string()
            },
            "actions": [
                {
                    "kind": "write_file",
                    "provider_owned": true,
                    "path": provider_root.join("linked-outside/settings.v1.json").display().to_string(),
                    "content": { "encoding": "utf8", "data": "SHOULD_NOT_OVERWRITE_OUTSIDE" }
                }
            ]
        }),
        "migration.schema.json#/$defs/MigrationApplyErrorResponse",
        "conflict",
    );

    assert_eq!(
        response["error"]["code"],
        "migration_action_outside_provider_root"
    );
    assert_file(&outside_file, "OUTSIDE_PROVIDER_SENTINEL");
}

#[test]
fn migration_apply_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("migration-apply-malformed-request");

    call_error(
        &roots,
        "migration.apply",
        json!(null),
        "migration.schema.json#/$defs/MigrationApplyErrorResponse",
    );
}
