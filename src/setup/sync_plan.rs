// declared_role: accessor, formatter, mapper, orchestration, parser, predicate, validator

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

struct PlannedOperation {
    operation: Value,
    diagnostic: Option<Value>,
}

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = sync_params(request)?;
    let home = required_home_dir(&request.host)?;
    let overwrite = params
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut operations = Vec::new();
    let mut diagnostics = Vec::new();

    append_skill_operations(params, &home, overwrite, &mut operations, &mut diagnostics);
    append_mcp_operations(params, &home, overwrite, &mut operations, &mut diagnostics);
    append_config_operation(params, &home, overwrite, &mut operations);

    Ok(sync_response(operations, diagnostics))
}

fn required_home_dir(host: &Value) -> Result<PathBuf, ProviderFailure> {
    home_dir(host).ok_or_else(invalid_params)
}

fn sync_params(
    request: &RequestEnvelope,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)
}

fn append_skill_operations(
    params: &serde_json::Map<String, Value>,
    home: &Path,
    overwrite: bool,
    operations: &mut Vec<Value>,
    diagnostics: &mut Vec<Value>,
) {
    for name in skill_names(params) {
        append_planned_operation(
            operations,
            diagnostics,
            skill_plan_item(home, name, overwrite),
        );
    }
}

fn skill_plan_item(home: &Path, name: &str, overwrite: bool) -> PlannedOperation {
    let path = skill_path(home, name);
    let exists = path_exists(&path);
    PlannedOperation {
        operation: skill_operation(name, &path, exists, overwrite),
        diagnostic: skill_conflict_diagnostic_if_needed(name, exists, overwrite),
    }
}

fn append_mcp_operations(
    params: &serde_json::Map<String, Value>,
    home: &Path,
    overwrite: bool,
    operations: &mut Vec<Value>,
    diagnostics: &mut Vec<Value>,
) {
    let config_path = mcp_config_path(home);
    let config = read_json(&config_path);
    for name in mcp_server_names(params) {
        append_planned_operation(
            operations,
            diagnostics,
            mcp_plan_item(name, &config_path, config.as_ref(), overwrite),
        );
    }
}

fn mcp_config_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

fn mcp_plan_item(
    name: &str,
    config_path: &Path,
    config: Option<&Value>,
    overwrite: bool,
) -> PlannedOperation {
    let exists = mcp_server_exists(config, name);
    PlannedOperation {
        operation: mcp_operation(name, config_path, exists, overwrite),
        diagnostic: mcp_conflict_diagnostic_if_needed(name, exists, overwrite),
    }
}

fn append_planned_operation(
    operations: &mut Vec<Value>,
    diagnostics: &mut Vec<Value>,
    item: PlannedOperation,
) {
    operations.push(item.operation);
    if let Some(diagnostic) = item.diagnostic {
        diagnostics.push(diagnostic);
    }
}

fn skill_conflict_diagnostic_if_needed(name: &str, exists: bool, overwrite: bool) -> Option<Value> {
    conflict_exists(exists, overwrite).then(|| skill_conflict_diagnostic(name))
}

fn mcp_conflict_diagnostic_if_needed(name: &str, exists: bool, overwrite: bool) -> Option<Value> {
    conflict_exists(exists, overwrite).then(|| mcp_conflict_diagnostic(name))
}

fn append_config_operation(
    params: &serde_json::Map<String, Value>,
    home: &Path,
    overwrite: bool,
    operations: &mut Vec<Value>,
) {
    let config_path = config_path(home);
    if config_requested(params) {
        operations.push(config_operation(
            &config_path,
            path_exists(&config_path),
            overwrite,
        ));
    }
}

fn config_requested(params: &serde_json::Map<String, Value>) -> bool {
    params.get("config").is_some()
}

fn config_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

fn skill_names(params: &serde_json::Map<String, Value>) -> Vec<&str> {
    named_items(params, "skills")
}

fn mcp_server_names(params: &serde_json::Map<String, Value>) -> Vec<&str> {
    named_items(params, "mcp_servers")
}

fn named_items<'a>(params: &'a serde_json::Map<String, Value>, key: &str) -> Vec<&'a str> {
    selected_named_items(params, key)
        .into_iter()
        .map(named_item_name)
        .collect()
}

fn selected_named_items<'a>(
    params: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Vec<&'a Value> {
    named_item_values(params, key)
        .into_iter()
        .filter(|item| has_named_item_name(item))
        .collect()
}

fn named_item_values<'a>(params: &'a serde_json::Map<String, Value>, key: &str) -> Vec<&'a Value> {
    params
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect()
}

fn named_item_name(item: &Value) -> &str {
    named_item_name_value(item).expect("selected named item has a name")
}

fn has_named_item_name(item: &Value) -> bool {
    named_item_name_value(item).is_some()
}

fn named_item_name_value(item: &Value) -> Option<&str> {
    item.get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

fn skill_path(home: &Path, name: &str) -> PathBuf {
    home.join(".claude")
        .join("skills")
        .join(name)
        .join("SKILL.md")
}

fn path_exists(path: &Path) -> bool {
    path.exists()
}

fn conflict_exists(exists: bool, overwrite: bool) -> bool {
    exists && !overwrite
}

fn mcp_server_exists(config: Option<&Value>, name: &str) -> bool {
    config
        .and_then(|value| value.get("mcpServers"))
        .and_then(|value| value.get(name))
        .is_some()
}

fn skill_operation(name: &str, path: &Path, exists: bool, overwrite: bool) -> Value {
    json!({
        "kind": "skill",
        "name": name,
        "path": path.display().to_string(),
        "action": skill_action(exists, overwrite),
        "mutates": false
    })
}

fn mcp_operation(name: &str, path: &Path, exists: bool, overwrite: bool) -> Value {
    json!({
        "kind": "mcp",
        "name": name,
        "path": path.display().to_string(),
        "action": mcp_action(exists, overwrite),
        "mutates": false
    })
}

fn config_operation(path: &Path, exists: bool, overwrite: bool) -> Value {
    json!({
        "kind": "config",
        "path": path.display().to_string(),
        "action": config_action(exists, overwrite),
        "mutates": false
    })
}

fn skill_action(exists: bool, overwrite: bool) -> &'static str {
    if exists && !overwrite {
        "would_skip_conflict"
    } else if exists {
        "would_overwrite"
    } else {
        "would_create"
    }
}

fn mcp_action(exists: bool, overwrite: bool) -> &'static str {
    if exists && !overwrite {
        "would_skip_conflict"
    } else if exists {
        "would_update"
    } else {
        "would_add"
    }
}

fn config_action(exists: bool, overwrite: bool) -> &'static str {
    if exists && !overwrite {
        "would_merge_without_overwrite"
    } else {
        "would_write"
    }
}

fn skill_conflict_diagnostic(name: &str) -> Value {
    diagnostic(
        "warning",
        &skill_diagnostic_path(name),
        "skill_conflict",
        "skill conflict: existing Claude skill would not be overwritten",
    )
}

fn mcp_conflict_diagnostic(name: &str) -> Value {
    diagnostic(
        "warning",
        &mcp_diagnostic_path(name),
        "mcp_conflict",
        "mcp conflict: existing Claude MCP server would not be overwritten",
    )
}

fn skill_diagnostic_path(name: &str) -> String {
    format!("skills.{name}")
}

fn mcp_diagnostic_path(name: &str) -> String {
    format!("mcp_servers.{name}")
}

fn sync_response(operations: Vec<Value>, diagnostics: Vec<Value>) -> Value {
    json!({
        "operations": operations,
        "diagnostics": diagnostics
    })
}

fn read_json(path: &Path) -> Option<Value> {
    read_json_bytes(path).and_then(|bytes| parse_json_bytes(&bytes))
}

fn read_json_bytes(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn parse_json_bytes(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(bytes).ok()
}

fn home_dir(host: &Value) -> Option<PathBuf> {
    home_path_buf(accepted_home_value(host))
}

fn accepted_home_value(host: &Value) -> Option<&str> {
    home_value(host).filter(non_empty_string)
}

fn home_path_buf(value: Option<&str>) -> Option<PathBuf> {
    value.map(path_buf_from_str)
}

fn home_value(host: &Value) -> Option<&str> {
    host.get("env")
        .and_then(|env| env.get("HOME"))
        .and_then(Value::as_str)
}

fn non_empty_string(value: &&str) -> bool {
    !value.is_empty()
}

fn path_buf_from_str(value: &str) -> PathBuf {
    PathBuf::from(value)
}

fn diagnostic(severity: &str, path: &str, code: &str, message: &str) -> Value {
    json!({
        "severity": severity,
        "path": path,
        "code": code,
        "message": message
    })
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_setup_sync_plan_params",
        "setup.sync_plan params do not match the setup contract",
    )
}
