// declared_role: orchestration, mapper, accessor, validator, predicate
// intrinsic_surface_declarations:
//   - component: tests/contract_migration.rs
//     role: intrinsic-surface
//     Domain: contract_migration_proof_surface
//     Owns:
//       - migration contract scenarios
//       - support harness dependencies for migration invoke/schema proof

mod support;

use agent_runner_claude::encoding::sha256_hex;
use serde_json::{json, Value};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

fn call(roots: &TempRoots, subcommand: &str, params: Value) -> Value {
    let output = invoke(subcommand, &contract_request(roots, params));
    assert_success_invocation(&output);
    stdout_json(&output)
}

fn call_error(roots: &TempRoots, subcommand: &str, params: Value, schema: &str) -> Value {
    call_error_category(roots, subcommand, params, schema, "invalid_request")
}

fn call_error_category(
    roots: &TempRoots,
    subcommand: &str,
    params: Value,
    schema: &str,
    category: &str,
) -> Value {
    let output = invoke(subcommand, &contract_request(roots, params));
    assert_error_invocation(&output, category);
    let response = stdout_json(&output);
    assert_error_response(schema, &response, category);
    response
}

fn contract_request(roots: &TempRoots, params: Value) -> Value {
    envelope(CONTRACT, host_context(roots), params)
}

fn assert_success_invocation(output: &support::invoke::Invocation) {
    assert_eq!(output.code, Some(0));
    assert_empty_stderr(output);
}

fn assert_error_invocation(output: &support::invoke::Invocation, category: &str) {
    assert_eq!(output.code, Some(expected_error_exit_code(category)));
    assert_empty_stderr(output);
}

fn expected_error_exit_code(category: &str) -> i32 {
    if category == "conflict" {
        1
    } else {
        2
    }
}

fn assert_empty_stderr(output: &support::invoke::Invocation) {
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout_json(output: &support::invoke::Invocation) -> Value {
    parse_one_stdout_json(output)
}

fn assert_error_response(schema: &str, response: &Value, category: &str) {
    assert_valid(schema, response);
    assert!(!response["ok"].as_bool().unwrap());
    assert_eq!(response["error"]["category"], category);
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).expect("write fixture");
}

fn assert_file(path: &Path, expected: &str) {
    assert_file_text(&read_fixture(path), expected);
}

fn read_fixture(path: &Path) -> String {
    fs::read_to_string(path).expect("read fixture")
}

fn assert_file_text(actual: &str, expected: &str) {
    assert_eq!(actual, expected);
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
    let central_journal = roots.data_root.join("central-state.sqlite-journal");
    fs::create_dir_all(provider_file.parent().unwrap()).unwrap();
    write(&legacy_config, "LEGACY_CONFIG_SENTINEL");
    write(&provider_file, "PROVIDER_FILE_SENTINEL");
    write(&central_state, "CENTRAL_STATE_PLAN_SENTINEL");
    write(&central_journal, "CENTRAL_JOURNAL_PLAN_SENTINEL");

    let response = call(
        &roots,
        "migration.plan",
        migration_plan_params(&roots, &central_state),
    );
    assert_migration_plan_response(&response);
    assert_migration_plan_sentinels(
        &legacy_config,
        &provider_file,
        &central_state,
        &central_journal,
    );
}

fn migration_plan_params(roots: &TempRoots, central_state: &Path) -> Value {
    json!({
        "from": "legacy-host-state",
        "to": "claude.settings/v1",
        "provider_root": roots.home.join(".claude").display().to_string(),
        "legacy": {
            "providers.toml": { "claude-primary": { "command": "claude" } },
            "sessions.toml": { "claude-primary": { "turn_script": "turns" } }
        },
        "central_state": central_state.display().to_string()
    })
}

fn assert_migration_plan_response(response: &Value) {
    assert_valid(
        "migration.schema.json#/$defs/MigrationPlanResponse",
        response,
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
}

fn assert_migration_plan_sentinels(
    legacy_config: &Path,
    provider_file: &Path,
    central_state: &Path,
    central_journal: &Path,
) {
    assert_file(legacy_config, "LEGACY_CONFIG_SENTINEL");
    assert_file(provider_file, "PROVIDER_FILE_SENTINEL");
    assert_file(central_state, "CENTRAL_STATE_PLAN_SENTINEL");
    assert_file(central_journal, "CENTRAL_JOURNAL_PLAN_SENTINEL");
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
    let central_state = roots.data_root.join("central-state.sqlite");
    let central_journal = roots.data_root.join("central-state.sqlite-journal");
    fs::create_dir_all(&hostile_root).expect("create hostile root");
    let sentinel = hostile_root.join("settings.v1.json");
    write(&sentinel, "HOSTILE_ROOT_SENTINEL");
    write(&central_state, "CENTRAL_STATE_REJECTED_PLAN_SENTINEL");
    write(&central_journal, "CENTRAL_JOURNAL_REJECTED_PLAN_SENTINEL");

    let response = call_error_category(
        &roots,
        "migration.plan",
        hostile_root_plan_params(&hostile_root, &central_state),
        "migration.schema.json#/$defs/MigrationPlanErrorResponse",
        "conflict",
    );
    assert_provider_root_rejection(&response, &sentinel, &central_state, &central_journal);
}

fn hostile_root_plan_params(hostile_root: &Path, central_state: &Path) -> Value {
    json!({
        "from": "legacy-host-state",
        "to": "claude.settings/v1",
        "provider_root": hostile_root.display().to_string(),
        "legacy": {
            "providers.toml": { "claude-primary": { "command": "claude" } },
            "sessions.toml": { "claude-primary": { "turn_script": "turns" } }
        },
        "central_state": central_state.display().to_string()
    })
}

fn assert_provider_root_rejection(
    response: &Value,
    sentinel: &Path,
    central_state: &Path,
    central_journal: &Path,
) {
    assert_eq!(
        response["error"]["code"],
        "migration_provider_root_outside_provider_root"
    );
    assert_file(sentinel, "HOSTILE_ROOT_SENTINEL");
    assert_file(central_state, "CENTRAL_STATE_REJECTED_PLAN_SENTINEL");
    assert_file(central_journal, "CENTRAL_JOURNAL_REJECTED_PLAN_SENTINEL");
}

#[test]
fn migration_apply_only_performs_confirmed_provider_owned_actions_and_leaves_host_central_state() {
    let roots = temp_roots("migration-apply");
    let provider_root = roots.home.join(".claude");
    fs::create_dir_all(&provider_root).unwrap();
    let central_state = roots.data_root.join("central-state.sqlite");
    let central_journal = roots.data_root.join("central-state.sqlite-journal");
    write(&central_state, "CENTRAL_STATE_SENTINEL_BEFORE_MIGRATION");
    write(
        &central_journal,
        "CENTRAL_JOURNAL_SENTINEL_BEFORE_MIGRATION",
    );
    let confirmed_provider_file = provider_root.join("settings.v1.json");
    let unconfirmed_provider_file = provider_root.join("unconfirmed.json");
    write(&unconfirmed_provider_file, "UNCONFIRMED_PROVIDER_SENTINEL");

    let response = call(
        &roots,
        "migration.apply",
        migration_apply_params(
            &roots,
            &provider_root,
            &central_state,
            &central_journal,
            &unconfirmed_provider_file,
        ),
    );
    assert_migration_apply_response(&response);
    assert_migration_apply_wrote_provider_file(
        &response,
        &confirmed_provider_file,
        "{\"provider\":\"claude\"}",
    );
    assert_migration_apply_sentinels(&central_state, &central_journal, &unconfirmed_provider_file);
}

fn migration_apply_params(
    roots: &TempRoots,
    provider_root: &Path,
    central_state: &Path,
    central_journal: &Path,
    unconfirmed_provider_file: &Path,
) -> Value {
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
                "provider_owned": false,
                "path": central_journal.display().to_string(),
                "content": { "encoding": "utf8", "data": "SHOULD_NOT_BE_WRITTEN_TO_JOURNAL" }
            },
            {
                "kind": "write_file",
                "provider_owned": true,
                "confirmed": false,
                "path": unconfirmed_provider_file.display().to_string(),
                "content": { "encoding": "utf8", "data": "SHOULD_NOT_OVERWRITE_UNCONFIRMED" }
            }
        ]
    })
}

fn assert_migration_apply_response(response: &Value) {
    assert_valid(
        "migration.schema.json#/$defs/MigrationApplyResponse",
        response,
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
}

fn assert_migration_apply_sentinels(
    central_state: &Path,
    central_journal: &Path,
    unconfirmed_provider_file: &Path,
) {
    assert_file(central_state, "CENTRAL_STATE_SENTINEL_BEFORE_MIGRATION");
    assert_file(central_journal, "CENTRAL_JOURNAL_SENTINEL_BEFORE_MIGRATION");
    assert_file(unconfirmed_provider_file, "UNCONFIRMED_PROVIDER_SENTINEL");
}

fn assert_migration_apply_wrote_provider_file(response: &Value, path: &Path, expected: &str) {
    assert_file(path, expected);

    let actual = fs::read(path).expect("read applied provider file");
    let expected_path = path.display().to_string();
    let artifact = response["result"]["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["path"].as_str() == Some(expected_path.as_str()))
        .unwrap_or_else(|| panic!("missing artifact metadata for {expected_path}"));
    assert_eq!(artifact["sha256"], sha256_hex(&actual));
}

#[test]
fn migration_apply_rejects_provider_owned_claim_for_host_path_and_leaves_sentinel_unchanged() {
    let roots = temp_roots("migration-apply-host-path-confinement");
    let central_state = roots.data_root.join("central-state.sqlite");
    let central_journal = roots.data_root.join("central-state.sqlite-journal");
    write(&central_state, "HOST_CENTRAL_STATE_SENTINEL");
    write(&central_journal, "HOST_CENTRAL_JOURNAL_SENTINEL");

    let response = call_error_category(
        &roots,
        "migration.apply",
        host_path_apply_params(&roots, &central_state, &central_journal),
        "migration.schema.json#/$defs/MigrationApplyErrorResponse",
        "conflict",
    );
    assert_host_path_apply_rejection(&response, &central_state, &central_journal);
}

fn host_path_apply_params(
    roots: &TempRoots,
    central_state: &Path,
    central_journal: &Path,
) -> Value {
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
                "path": central_journal.display().to_string(),
                "content": { "encoding": "utf8", "data": "SHOULD_NOT_OVERWRITE_HOST_JOURNAL" }
            },
            {
                "kind": "write_file",
                "provider_owned": true,
                "path": central_state.display().to_string(),
                "content": { "encoding": "utf8", "data": "SHOULD_NOT_OVERWRITE_HOST_STATE" }
            }
        ]
    })
}

fn assert_host_path_apply_rejection(
    response: &Value,
    central_state: &Path,
    central_journal: &Path,
) {
    assert_eq!(
        response["error"]["code"],
        "migration_action_outside_provider_root"
    );
    assert_file(central_state, "HOST_CENTRAL_STATE_SENTINEL");
    assert_file(central_journal, "HOST_CENTRAL_JOURNAL_SENTINEL");
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
        symlink_apply_params(&roots, &provider_root),
        "migration.schema.json#/$defs/MigrationApplyErrorResponse",
        "conflict",
    );
    assert_symlink_apply_rejection(&response, &outside_file);
}

#[cfg(unix)]
fn symlink_apply_params(roots: &TempRoots, provider_root: &Path) -> Value {
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
    })
}

#[cfg(unix)]
fn assert_symlink_apply_rejection(response: &Value, outside_file: &Path) {
    assert_eq!(
        response["error"]["code"],
        "migration_action_outside_provider_root"
    );
    assert_file(outside_file, "OUTSIDE_PROVIDER_SENTINEL");
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
