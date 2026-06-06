// declared_role: orchestration, mapper, accessor, validator, predicate, formatter
// intrinsic_surface_declarations:
//   - component: tests/contract_rotation.rs
//     role: intrinsic-surface
//     Domain: contract_rotation_proof_surface
//     Owns:
//       - rotation contract scenarios
//       - support harness dependencies for rotation invoke/schema proof

mod support;

use agent_runner_claude::encoding::sha256_hex;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use support::fixtures::{envelope, host_context, temp_roots, TempRoots, CONTRACT};
use support::invoke::{invoke, parse_one_stdout_json};
use support::schema::assert_valid;

const MATERIALIZE_CANONICAL_BYTES: &[u8] = br#"{"turn":1}"#;

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

fn write_sentinel(path: &Path, text: &str) {
    fs::write(path, text).expect("write sentinel");
}

fn assert_sentinel(path: &Path, expected: &str) {
    assert_sentinel_text(path, &sentinel_text(path), expected);
}

fn sentinel_text(path: &Path) -> String {
    fs::read_to_string(path).expect("read sentinel")
}

fn assert_sentinel_text(path: &Path, actual: &str, expected: &str) {
    assert_eq!(actual, expected, "sentinel changed at {}", path.display());
}

fn has_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => text.contains(needle),
        Value::Array(items) => items.iter().any(|item| has_string(item, needle)),
        Value::Object(map) => map.values().any(|item| has_string(item, needle)),
        _ => false,
    }
}

fn assert_sha256(text: &str) {
    assert_eq!(text.len(), 64, "sha256 digest must be 64 hex chars: {text}");
    assert!(text
        .chars()
        .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
}

#[test]
fn rotation_assess_reports_decision_metadata_and_never_mutates_host_state() {
    let roots = temp_roots("rotation-assess");
    let host_db = roots.data_root.join("host.sqlite");
    let host_journal = roots.data_root.join("host.sqlite-journal");
    write_sentinel(&host_db, "HOST_DB_SENTINEL_W2C");
    write_sentinel(&host_journal, "HOST_JOURNAL_SENTINEL_W2C");

    let response = call(
        &roots,
        "rotation.assess",
        assess_params(&host_db, &host_journal),
    );
    assert_assess_response(&response);
    assert_assess_sentinels(&host_db, &host_journal);
}

fn assess_params(host_db: &Path, host_journal: &Path) -> Value {
    json!({
        "source_provider": "claude",
        "target_provider": "claude",
        "source_session_id": "source-session-001",
        "target_session_id": "target-session-001",
        "transition_reason": "quota_threshold",
        "host_state": {
            "sqlite_path": host_db.display().to_string(),
            "journal_path": host_journal.display().to_string()
        },
        "quota": {
            "terminal_signal": "maybe_quota_exhausted",
            "remaining_percent": 3
        }
    })
}

fn assert_assess_response(response: &Value) {
    assert_valid(
        "rotation.schema.json#/$defs/RotationAssessResponse",
        response,
    );
    assert!(response["ok"].as_bool().unwrap());

    let result = &response["result"];
    assert!(result["allowed"].is_boolean());
    assert!(
        result["score"].is_i64(),
        "rotation.assess must include a score"
    );
    assert!(!result["reason"].as_str().unwrap_or_default().is_empty());
    assert!(!result["requirements"].as_array().unwrap().is_empty());
}

fn assert_assess_sentinels(host_db: &Path, host_journal: &Path) {
    assert_sentinel(host_db, "HOST_DB_SENTINEL_W2C");
    assert_sentinel(host_journal, "HOST_JOURNAL_SENTINEL_W2C");
}

#[test]
fn rotation_assess_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("rotation-assess-malformed-request");

    call_error(
        &roots,
        "rotation.assess",
        json!(null),
        "rotation.schema.json#/$defs/RotationAssessErrorResponse",
    );
}

#[test]
fn rotation_materialize_returns_provider_artifacts_plan_and_does_not_apply_host_db_or_journal() {
    let roots = temp_roots("rotation-materialize");
    let host_db = roots.data_root.join("central-state.sqlite");
    let host_journal = roots.data_root.join("central-state.sqlite-journal");
    write_sentinel(&host_db, "CENTRAL_DB_BEFORE_ROTATION");
    write_sentinel(&host_journal, "CENTRAL_JOURNAL_BEFORE_ROTATION");

    let target_session_id = "target-session-materialized-001";
    let response = call(
        &roots,
        "rotation.materialize",
        materialize_params(&roots, &host_db, &host_journal, target_session_id),
    );
    assert_materialize_response(&response, &host_db, &host_journal, target_session_id);
    assert_materialized_artifact_files(
        &response,
        &host_db,
        &host_journal,
        MATERIALIZE_CANONICAL_BYTES,
    );
    assert_materialize_sentinels(&host_db, &host_journal);
}

fn materialize_params(
    roots: &TempRoots,
    host_db: &Path,
    host_journal: &Path,
    target_session_id: &str,
) -> Value {
    json!({
        "chain_id": "chain-w2c-rotation",
        "source_provider": "claude",
        "target_provider": "claude",
        "source_session_id": "source-session-materialize-001",
        "target_session_id": target_session_id,
        "transition_reason": "exhausted",
        "provider_artifact_root": roots.data_root.join("claude").display().to_string(),
        "canonical_transcript": {
            "kind": "bytes",
            "data_base64": "eyJ0dXJuIjoxfQ=="
        },
        "host_state": {
            "sqlite_path": host_db.display().to_string(),
            "journal_path": host_journal.display().to_string()
        }
    })
}

fn assert_materialize_response(
    response: &Value,
    host_db: &Path,
    host_journal: &Path,
    target_session_id: &str,
) {
    assert_valid(
        "rotation.schema.json#/$defs/RotationMaterializeResponse",
        response,
    );
    assert!(response["ok"].as_bool().unwrap());

    let result = &response["result"];
    assert!(result["changed"].as_bool().unwrap());
    assert_eq!(result["target_provider_session_id"], target_session_id);

    let artifacts = result["artifacts"].as_array().unwrap();
    assert!(
        !artifacts.is_empty(),
        "materialize must return provider-owned artifacts"
    );
    for artifact in artifacts {
        assert!(artifact["path"]
            .as_str()
            .unwrap_or_default()
            .contains("claude"));
        assert_sha256(artifact["sha256"].as_str().expect("artifact sha256"));
        assert_ne!(artifact["path"], host_db.display().to_string());
        assert_ne!(artifact["path"], host_journal.display().to_string());
    }

    let plan = &result["host_state_plan"];
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["operation"], "rotation.materialize");
    assert_eq!(plan["target_session_id"], target_session_id);
    assert!(has_string(&plan["segments"], target_session_id));
    for artifact in plan["artifacts"].as_array().unwrap() {
        assert_eq!(artifact["kind"], "file");
        assert!(artifact["path"].as_str().unwrap().contains("claude"));
        assert_sha256(artifact["sha256"].as_str().unwrap());
    }
}

fn assert_materialize_sentinels(host_db: &Path, host_journal: &Path) {
    assert_sentinel(host_db, "CENTRAL_DB_BEFORE_ROTATION");
    assert_sentinel(host_journal, "CENTRAL_JOURNAL_BEFORE_ROTATION");
}

fn assert_materialized_artifact_files(
    response: &Value,
    host_db: &Path,
    host_journal: &Path,
    expected_bytes: &[u8],
) {
    for artifact in response["result"]["artifacts"].as_array().unwrap() {
        let path = artifact["path"].as_str().expect("artifact path");
        assert_ne!(path, host_db.display().to_string());
        assert_ne!(path, host_journal.display().to_string());

        let bytes = fs::read(path)
            .unwrap_or_else(|error| panic!("read materialized rotation artifact {path}: {error}"));
        assert_eq!(bytes, expected_bytes, "artifact bytes changed at {path}");
        assert_eq!(artifact["sha256"], sha256_hex(&bytes));
    }
}

#[test]
fn rotation_materialize_rejects_artifact_root_that_escapes_provider_root_without_host_writes() {
    let roots = temp_roots("rotation-materialize-host-path-confinement");
    let escaped_root = roots.root.join("host-owned-artifacts");
    fs::create_dir_all(&escaped_root).expect("create escaped root");
    let escaped_sentinel = escaped_root.join("sentinel.txt");
    write_sentinel(&escaped_sentinel, "ESCAPED_ROOT_SENTINEL");
    let host_db = roots.data_root.join("central-state.sqlite");
    let host_journal = roots.data_root.join("central-state.sqlite-journal");
    write_sentinel(&host_db, "CENTRAL_DB_BEFORE_REJECTED_ROTATION");
    write_sentinel(&host_journal, "CENTRAL_JOURNAL_BEFORE_REJECTED_ROTATION");

    let chain_id = "chain-host-path-adversarial";
    let target_session_id = "target-host-path-adversarial";
    let response = call_error_category(
        &roots,
        "rotation.materialize",
        escaped_root_materialize_params(
            chain_id,
            target_session_id,
            &escaped_root,
            &host_db,
            &host_journal,
        ),
        "rotation.schema.json#/$defs/RotationMaterializeErrorResponse",
        "conflict",
    );
    assert_escaped_root_rejection(
        &response,
        &escaped_sentinel,
        &host_db,
        &host_journal,
        &escaped_root,
        chain_id,
        target_session_id,
    );
}

fn escaped_root_materialize_params(
    chain_id: &str,
    target_session_id: &str,
    escaped_root: &Path,
    host_db: &Path,
    host_journal: &Path,
) -> Value {
    json!({
        "chain_id": chain_id,
        "source_provider": "claude",
        "target_provider": "claude",
        "source_session_id": "source-host-path-adversarial",
        "target_session_id": target_session_id,
        "transition_reason": "manual",
        "provider_artifact_root": escaped_root.display().to_string(),
        "canonical_transcript": {
            "kind": "bytes",
            "data_base64": "eyJ0dXJuIjoxfQ=="
        },
        "host_state": {
            "sqlite_path": host_db.display().to_string(),
            "journal_path": host_journal.display().to_string()
        }
    })
}

fn assert_escaped_root_rejection(
    response: &Value,
    escaped_sentinel: &Path,
    host_db: &Path,
    host_journal: &Path,
    escaped_root: &Path,
    chain_id: &str,
    target_session_id: &str,
) {
    assert_eq!(
        response["error"]["code"],
        "rotation_artifact_root_outside_provider_root"
    );
    assert_sentinel(escaped_sentinel, "ESCAPED_ROOT_SENTINEL");
    assert_sentinel(host_db, "CENTRAL_DB_BEFORE_REJECTED_ROTATION");
    assert_sentinel(host_journal, "CENTRAL_JOURNAL_BEFORE_REJECTED_ROTATION");
    assert!(
        !escaped_root
            .join("rotation")
            .join(format!("{chain_id}-{target_session_id}.canonical.jsonl"))
            .exists(),
        "rejected rotation must not materialize artifacts outside the provider root"
    );
}

#[test]
fn rotation_materialize_rejects_filename_segment_traversal_without_host_writes() {
    let roots = temp_roots("rotation-materialize-segment-traversal");
    let provider_root = roots.data_root.join("claude");
    let host_sentinel = roots.data_root.join("host-sentinel.jsonl");
    write_sentinel(&host_sentinel, "HOST_SENTINEL_BEFORE_SEGMENT_REJECT");

    let response = call_error(
        &roots,
        "rotation.materialize",
        traversal_materialize_params(&roots, &provider_root),
        "rotation.schema.json#/$defs/RotationMaterializeErrorResponse",
    );
    assert_traversal_rejection(&response, &host_sentinel, &provider_root);
}

fn traversal_materialize_params(roots: &TempRoots, provider_root: &Path) -> Value {
    json!({
        "chain_id": "../host-sentinel",
        "source_provider": "claude",
        "target_provider": "claude",
        "source_session_id": "source-segment-adversarial",
        "target_session_id": "target-segment-adversarial",
        "transition_reason": "manual",
        "provider_artifact_root": provider_root.display().to_string(),
        "canonical_transcript": {
            "kind": "bytes",
            "data_base64": "eyJ0dXJuIjoxfQ=="
        },
        "host_state": {
            "sqlite_path": roots.data_root.join("central-state.sqlite").display().to_string(),
            "journal_path": roots.data_root.join("central-state.sqlite-journal").display().to_string()
        }
    })
}

fn assert_traversal_rejection(response: &Value, host_sentinel: &Path, provider_root: &Path) {
    assert_eq!(
        response["error"]["code"],
        "invalid_rotation_filename_segment"
    );
    assert_sentinel(host_sentinel, "HOST_SENTINEL_BEFORE_SEGMENT_REJECT");
    assert!(
        !provider_root
            .join("host-sentinel-target-segment-adversarial.canonical.jsonl")
            .exists(),
        "rejected filename traversal must not materialize a normalized artifact"
    );
}

#[test]
fn rotation_materialize_malformed_request_uses_capability_error_def() {
    let roots = temp_roots("rotation-materialize-malformed-request");

    call_error(
        &roots,
        "rotation.materialize",
        json!(null),
        "rotation.schema.json#/$defs/RotationMaterializeErrorResponse",
    );
}
