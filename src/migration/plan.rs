// declared_role: accessor, filter, formatter, mapper, orchestration, validator

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = plan_params(request)?;
    let provider_root = provider_root(request, params)?;
    let to = target_settings_schema(params);
    Ok(plan_response(&provider_root, to))
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
    if let Some(root) = explicit_provider_root(params) {
        return confined_provider_root(&root, &roots);
    }
    confined_provider_root(&crate::fs::paths::provider_data_dir(&request.host)?, &roots)
}

fn explicit_provider_root(params: &serde_json::Map<String, Value>) -> Option<String> {
    params
        .get("provider_root")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn provider_roots(request: &RequestEnvelope) -> Result<Vec<PathBuf>, ProviderFailure> {
    let base = crate::fs::paths::host_data_root(&request.host)?;
    let mut roots = vec![crate::fs::paths::normalized_absolute(
        &base.join("claude"),
        &base,
    )];
    if let Some(root) = claude_home_root(&request.host) {
        roots.push(crate::fs::paths::normalized_absolute(&root, &base));
    }
    Ok(roots)
}

fn claude_home_root(host: &Value) -> Option<PathBuf> {
    host.get("env")
        .and_then(|env| env.get("HOME"))
        .and_then(Value::as_str)
        .filter(|home| !home.is_empty())
        .map(|home| Path::new(home).join(".claude"))
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
    provider_roots
        .iter()
        .find_map(|provider_root| {
            crate::fs::paths::confined_path_or_root(provider_root, requested_root).ok()
        })
        .ok_or_else(|| outside_provider_root(requested_root, provider_roots))
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

fn plan_response(provider_root: &str, to: &str) -> Value {
    json!({
        "actions": [
            super::legacy::planned_write(provider_root),
            {
                "kind": "backup_provider_file",
                "provider_owned": true,
                "path": format!("{provider_root}/settings.v1.json"),
                "description": "backup existing Claude provider settings before migration"
            }
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
