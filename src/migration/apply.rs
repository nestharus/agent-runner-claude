// declared_role: accessor, filter, formatter, mapper, orchestration, parser, predicate, validator
// adapter_declarations:
//   - component: src/migration/apply.rs
//     role: adapter
//     Translates:
//       - contract/v1/migration.schema.json#/$defs/MigrationApplyRequest
//       - contract/v1/migration.schema.json#/$defs/MigrationApplyResult
//       - src/fs/paths.rs provider-root confinement seam
//       - src/fs/atomic.rs provider-owned atomic file-write seam
//       - src/encoding.rs migration payload base64/hash seam

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

struct MigrationAction {
    path: PathBuf,
    bytes: Vec<u8>,
}

struct ApplyRequest<'a> {
    globally_confirmed: bool,
    actions: &'a [Value],
    provider_roots: ProviderRoots,
}

struct AppliedProviderWrite {
    action: Value,
    artifact: Value,
}

struct ProviderRoots {
    roots: Vec<PathBuf>,
}

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request = apply_request(request)?;
    let mut applied_actions = Vec::new();
    let mut artifacts = Vec::new();
    let mut warnings = Vec::new();

    let writes = confirmed_provider_writes(
        request.actions,
        request.globally_confirmed,
        &request.provider_roots,
        &mut warnings,
    )?;
    append_applied_provider_writes(
        &mut applied_actions,
        &mut artifacts,
        apply_provider_writes(writes)?,
    );

    Ok(apply_response(applied_actions, artifacts, warnings))
}

fn apply_request(request: &RequestEnvelope) -> Result<ApplyRequest<'_>, ProviderFailure> {
    let params = apply_params(request)?;
    let globally_confirmed = globally_confirmed(params);
    let actions = actions(params)?;
    let provider_roots = provider_roots(request)?;
    Ok(apply_request_value(
        globally_confirmed,
        actions,
        provider_roots,
    ))
}

fn apply_params(
    request: &RequestEnvelope,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)
}

fn apply_request_value(
    globally_confirmed: bool,
    actions: &[Value],
    provider_roots: ProviderRoots,
) -> ApplyRequest<'_> {
    ApplyRequest {
        globally_confirmed,
        actions,
        provider_roots,
    }
}

fn apply_provider_writes(
    writes: Vec<MigrationAction>,
) -> Result<Vec<AppliedProviderWrite>, ProviderFailure> {
    writes.into_iter().map(apply_provider_write).collect()
}

fn apply_provider_write(action: MigrationAction) -> Result<AppliedProviderWrite, ProviderFailure> {
    write_provider_file(&action.path, &action.bytes)?;
    Ok(applied_provider_write(&action.path, &action.bytes))
}

fn applied_provider_write(path: &Path, bytes: &[u8]) -> AppliedProviderWrite {
    AppliedProviderWrite {
        action: applied_action(path),
        artifact: file_artifact(path, bytes),
    }
}

fn append_applied_provider_writes(
    applied_actions: &mut Vec<Value>,
    artifacts: &mut Vec<Value>,
    writes: Vec<AppliedProviderWrite>,
) {
    for write in writes {
        applied_actions.push(write.action);
        artifacts.push(write.artifact);
    }
}

fn globally_confirmed(params: &serde_json::Map<String, Value>) -> bool {
    params
        .get("confirmed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && params
            .get("confirmation")
            .and_then(|value| value.get("accepted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn actions(params: &serde_json::Map<String, Value>) -> Result<&[Value], ProviderFailure> {
    params
        .get("actions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(invalid_params)
}

fn confirmed_provider_writes(
    actions: &[Value],
    globally_confirmed: bool,
    provider_roots: &ProviderRoots,
    warnings: &mut Vec<Value>,
) -> Result<Vec<MigrationAction>, ProviderFailure> {
    let confirmed = confirmed_write_actions(actions, globally_confirmed);
    let skipped = skipped_write_actions(actions, globally_confirmed);
    append_skipped_warnings(warnings, &skipped);
    provider_writes_from_actions(&confirmed, provider_roots)
}

fn confirmed_write_actions(actions: &[Value], globally_confirmed: bool) -> Vec<&Value> {
    actions
        .iter()
        .filter(|action| globally_confirmed && declares_confirmed_provider_write(action))
        .collect()
}

fn skipped_write_actions(actions: &[Value], globally_confirmed: bool) -> Vec<&Value> {
    actions
        .iter()
        .filter(|action| !globally_confirmed || !declares_confirmed_provider_write(action))
        .collect()
}

fn append_skipped_warnings(warnings: &mut Vec<Value>, skipped: &[&Value]) {
    warnings.extend(skipped.iter().map(|action| skipped_action(action)));
}

fn provider_writes_from_actions(
    actions: &[&Value],
    provider_roots: &ProviderRoots,
) -> Result<Vec<MigrationAction>, ProviderFailure> {
    actions
        .iter()
        .map(|action| confirmed_provider_write(action, provider_roots))
        .collect()
}

fn declares_confirmed_provider_write(action: &Value) -> bool {
    action.get("kind").and_then(Value::as_str) == Some("write_file")
        && action
            .get("provider_owned")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        && action
            .get("confirmed")
            .and_then(Value::as_bool)
            .unwrap_or(true)
}

fn confirmed_provider_write(
    action: &Value,
    provider_roots: &ProviderRoots,
) -> Result<MigrationAction, ProviderFailure> {
    let path = action_path(action)?;
    let confined_path = confined_provider_path(&path, provider_roots)?;
    let bytes = action_content(action)?;
    Ok(migration_action(confined_path, bytes))
}

fn migration_action(path: PathBuf, bytes: Vec<u8>) -> MigrationAction {
    MigrationAction { path, bytes }
}

fn action_path(action: &Value) -> Result<PathBuf, ProviderFailure> {
    action
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(invalid_params)
}

fn provider_roots(request: &RequestEnvelope) -> Result<ProviderRoots, ProviderFailure> {
    let base = host_data_root(&request.host)?;
    let home_root = claude_home_root(&request.host);
    Ok(provider_roots_value(&base, home_root))
}

fn provider_roots_value(base: &Path, home_root: Option<PathBuf>) -> ProviderRoots {
    ProviderRoots {
        roots: provider_root_paths(base, home_root),
    }
}

fn provider_root_paths(base: &Path, home_root: Option<PathBuf>) -> Vec<PathBuf> {
    let mut roots = vec![normalized_provider_data_root(base)];
    roots.extend(normalized_home_root(home_root.as_deref(), base));
    roots
}

fn normalized_provider_data_root(base: &Path) -> PathBuf {
    crate::fs::paths::normalized_absolute(&base.join("claude"), base)
}

fn normalized_home_root(root: Option<&Path>, base: &Path) -> Option<PathBuf> {
    root.map(|root| crate::fs::paths::normalized_absolute(root, base))
}

fn host_data_root(host: &Value) -> Result<PathBuf, ProviderFailure> {
    crate::fs::paths::host_data_root(host)
}

fn claude_home_root(host: &Value) -> Option<PathBuf> {
    accepted_home_value(home_value(host)).map(claude_home_path)
}

fn home_value(host: &Value) -> Option<&str> {
    host.get("env")
        .and_then(|env| env.get("HOME"))
        .and_then(Value::as_str)
}

fn accepted_home_value(home: Option<&str>) -> Option<&str> {
    home.filter(|home| !home.is_empty())
}

fn claude_home_path(home: &str) -> PathBuf {
    Path::new(home).join(".claude")
}

fn confined_provider_path(
    path: &Path,
    provider_roots: &ProviderRoots,
) -> Result<PathBuf, ProviderFailure> {
    selected_confined_provider_path(path, &provider_roots.roots)
        .ok_or_else(|| outside_provider_root(path, &provider_roots.roots))
}

fn selected_confined_provider_path(path: &Path, provider_roots: &[PathBuf]) -> Option<PathBuf> {
    provider_roots
        .iter()
        .find_map(|provider_root| crate::fs::paths::confined_child_path(provider_root, path).ok())
}

fn action_content(action: &Value) -> Result<Vec<u8>, ProviderFailure> {
    let fields = action_content_fields(action)?;
    decode_action_content(fields)
}

struct ActionContentFields<'a> {
    encoding: &'a str,
    data: &'a str,
}

enum ActionContentEncoding {
    Utf8,
    Base64,
}

fn action_content_fields(action: &Value) -> Result<ActionContentFields<'_>, ProviderFailure> {
    let content = action_content_object(action)?;
    let encoding = action_content_encoding(content)?;
    let data = action_content_data(content)?;
    Ok(action_content_fields_value(encoding, data))
}

fn action_content_fields_value<'a>(encoding: &'a str, data: &'a str) -> ActionContentFields<'a> {
    ActionContentFields { encoding, data }
}

fn action_content_object(
    action: &Value,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    action
        .get("content")
        .and_then(Value::as_object)
        .ok_or_else(invalid_params)
}

fn action_content_encoding(
    content: &serde_json::Map<String, Value>,
) -> Result<&str, ProviderFailure> {
    content
        .get("encoding")
        .and_then(Value::as_str)
        .ok_or_else(invalid_params)
}

fn action_content_data(content: &serde_json::Map<String, Value>) -> Result<&str, ProviderFailure> {
    content
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(invalid_params)
}

fn decode_action_content(fields: ActionContentFields<'_>) -> Result<Vec<u8>, ProviderFailure> {
    decode_action_content_data(validated_content_encoding(fields.encoding)?, fields.data)
}

fn validated_content_encoding(encoding: &str) -> Result<ActionContentEncoding, ProviderFailure> {
    match encoding {
        "utf8" => Ok(ActionContentEncoding::Utf8),
        "base64" => Ok(ActionContentEncoding::Base64),
        _ => Err(invalid_params()),
    }
}

fn decode_action_content_data(
    encoding: ActionContentEncoding,
    data: &str,
) -> Result<Vec<u8>, ProviderFailure> {
    match encoding {
        ActionContentEncoding::Utf8 => Ok(utf8_action_content_data(data)),
        ActionContentEncoding::Base64 => base64_action_content_data(data),
    }
}

fn utf8_action_content_data(data: &str) -> Vec<u8> {
    data.as_bytes().to_vec()
}

fn base64_action_content_data(data: &str) -> Result<Vec<u8>, ProviderFailure> {
    crate::encoding::decode_base64(data).map_err(invalid_base64_action_content)
}

fn invalid_base64_action_content(error: impl std::fmt::Display) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_migration_action_content",
        format!("migration action content is invalid base64: {error}"),
    )
}

fn write_provider_file(path: &Path, bytes: &[u8]) -> Result<(), ProviderFailure> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_failed("create", path, error))?;
    }
    crate::fs::atomic::atomic_write_bytes(path, bytes)
        .map_err(|error| io_failed("write", path, error))
}

fn file_artifact(path: &Path, bytes: &[u8]) -> Value {
    json!({
        "kind": "file",
        "path": path.display().to_string(),
        "sha256": crate::encoding::sha256_hex(bytes)
    })
}

fn applied_action(path: &Path) -> Value {
    json!({
        "kind": "write_file",
        "provider_owned": true,
        "path": path.display().to_string(),
        "status": "applied"
    })
}

fn skipped_action(action: &Value) -> Value {
    json!({
        "skipped": action.get("path").and_then(Value::as_str).unwrap_or("unknown"),
        "reason": "action was not both confirmed and provider-owned"
    })
}

fn apply_response(
    applied_actions: Vec<Value>,
    artifacts: Vec<Value>,
    warnings: Vec<Value>,
) -> Value {
    let applied_count = applied_actions.len();
    json!({
        "applied_actions": applied_actions,
        "artifacts": artifacts,
        "warnings": warnings,
        "outcome": {
            "status": "applied",
            "applied_count": applied_count
        }
    })
}

fn outside_provider_root(path: &Path, provider_roots: &[PathBuf]) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Conflict,
        "migration_action_outside_provider_root",
        format!(
            "migration action target {} is outside provider-owned roots {}",
            path.display(),
            provider_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        false,
    )
}

fn io_failed(action: &str, path: &Path, error: std::io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "migration_apply_io_failed",
        format!(
            "failed to {action} provider-owned migration file {}: {error}",
            path.display()
        ),
        true,
    )
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_migration_apply_params",
        "migration.apply params do not match the migration contract",
    )
}
