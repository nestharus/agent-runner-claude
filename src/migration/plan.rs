// declared_role: accessor, filter, formatter, mapper, orchestration, validator

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = plan_params(request)?;
    let provider_root = provider_root(request, params)?;
    let to = target_settings_schema(params);
    let content = planned_settings_content(params, to);
    Ok(plan_response(&provider_root, to, &content))
}

fn plan_params(
    request: &RequestEnvelope,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)
}

fn provider_root(
    request: &RequestEnvelope,
    params: &serde_json::Map<String, Value>,
) -> Result<String, ProviderFailure> {
    let roots = provider_roots(request)?;
    let requested = selected_provider_root(request, params)?;
    confined_provider_root(&requested, &roots)
}

fn selected_provider_root(
    request: &RequestEnvelope,
    params: &serde_json::Map<String, Value>,
) -> Result<PathBuf, ProviderFailure> {
    match explicit_provider_root(params) {
        Some(root) => Ok(PathBuf::from(root)),
        None => default_provider_root(request),
    }
}

fn default_provider_root(request: &RequestEnvelope) -> Result<PathBuf, ProviderFailure> {
    crate::fs::paths::provider_data_dir(&request.host)
}

fn explicit_provider_root(params: &serde_json::Map<String, Value>) -> Option<String> {
    accepted_provider_root(provider_root_value(params)).map(owned_provider_root)
}

fn provider_root_value(params: &serde_json::Map<String, Value>) -> Option<&str> {
    params.get("provider_root").and_then(Value::as_str)
}

fn accepted_provider_root(root: Option<&str>) -> Option<&str> {
    root.filter(|value| !value.is_empty())
}

fn owned_provider_root(root: &str) -> String {
    root.to_string()
}

fn provider_roots(request: &RequestEnvelope) -> Result<Vec<PathBuf>, ProviderFailure> {
    let base = crate::fs::paths::host_data_root(&request.host)?;
    Ok(provider_root_paths(&base, claude_home_root(&request.host)))
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

fn confined_provider_root(
    requested_root: impl AsRef<Path>,
    provider_roots: &[PathBuf],
) -> Result<String, ProviderFailure> {
    let requested_root = requested_root.as_ref();
    let root = confined_provider_root_path(requested_root, provider_roots)?;
    Ok(display_path(&root))
}

fn confined_provider_root_path(
    requested_root: &Path,
    provider_roots: &[PathBuf],
) -> Result<PathBuf, ProviderFailure> {
    selected_confined_provider_root(requested_root, provider_roots)
        .ok_or_else(|| outside_provider_root(requested_root, provider_roots))
}

fn selected_confined_provider_root(
    requested_root: &Path,
    provider_roots: &[PathBuf],
) -> Option<PathBuf> {
    provider_roots.iter().find_map(|provider_root| {
        crate::fs::paths::confined_path_or_root(provider_root, requested_root).ok()
    })
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

fn target_settings_schema(params: &serde_json::Map<String, Value>) -> &str {
    params
        .get("to")
        .and_then(Value::as_str)
        .unwrap_or("claude.settings/v1")
}

fn planned_settings_content(params: &serde_json::Map<String, Value>, schema_id: &str) -> Value {
    json!({
        "schema_id": schema_id,
        "records": legacy_settings_records(params)
    })
}

fn legacy_settings_records(params: &serde_json::Map<String, Value>) -> Vec<Value> {
    legacy_provider_entries(params)
        .into_iter()
        .map(legacy_settings_record)
        .collect()
}

fn legacy_provider_entries(params: &serde_json::Map<String, Value>) -> Vec<(&String, &Value)> {
    params
        .get("legacy")
        .and_then(|legacy| legacy.get("providers.toml"))
        .and_then(Value::as_object)
        .map(|providers| providers.iter().collect())
        .unwrap_or_default()
}

fn legacy_settings_record((name, values): (&String, &Value)) -> Value {
    json!({
        "id": name,
        "display_name": name,
        "values": values
    })
}

fn plan_response(provider_root: &str, to: &str, content: &Value) -> Value {
    json!({
        "actions": [
            super::legacy::planned_write(provider_root, content),
            backup_action(provider_root)
        ],
        "warnings": [
            "Create and verify a backup before applying provider-owned migration actions.",
            "Host central state is not mutated by migration.plan."
        ],
        "requires_backup": true,
        "confirmation": {
            "id": format!("confirm-claude-migration-to-{to}"),
            "message": "Confirm only provider-owned Claude file actions; host central state remains host-owned.",
            "provider_owned_only": true
        }
    })
}

fn backup_action(provider_root: &str) -> Value {
    json!({
        "kind": "backup_provider_file",
        "provider_owned": true,
        "confirmed": true,
        "path": format!("{provider_root}/settings.v1.json"),
        "content": { "encoding": "utf8", "data": "" },
        "description": "backup existing Claude provider settings before migration"
    })
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_migration_plan_params",
        "migration.plan params do not match the migration contract",
    )
}

fn outside_provider_root(path: &Path, provider_roots: &[PathBuf]) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Conflict,
        "migration_provider_root_outside_provider_root",
        format!(
            "migration provider root {} is outside provider-owned roots {}",
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
