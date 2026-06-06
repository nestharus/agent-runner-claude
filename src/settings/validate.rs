// declared_role: accessor, formatter, mapper, orchestration, validator

use serde_json::{json, Map, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let values = validate_params(&request.params)?;
    Ok(validate_response(validate_values(values)))
}

pub fn for_create(value: &Value) -> Result<(Option<String>, Value, Vec<Value>), ProviderFailure> {
    Ok(create_validation_result(create_params(value)?))
}

fn create_params(value: &Value) -> Result<&Map<String, Value>, ProviderFailure> {
    let params = required_object(value, "settings.create")?;
    ensure_create_params(params)?;
    Ok(params)
}

fn ensure_create_params(params: &Map<String, Value>) -> Result<(), ProviderFailure> {
    if !matches!(params.len(), 1 | 2)
        || !params.contains_key("values")
        || !params["values"].is_object()
        || params
            .get("display_name")
            .is_some_and(|value| !value.is_string())
    {
        return Err(invalid_params("settings.create"));
    }
    Ok(())
}

fn create_validation_result(params: &Map<String, Value>) -> (Option<String>, Value, Vec<Value>) {
    let values = create_values(params);
    (
        create_display_name(params),
        values.clone(),
        validate_values(&values),
    )
}

fn create_display_name(params: &Map<String, Value>) -> Option<String> {
    params
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn create_values(params: &Map<String, Value>) -> Value {
    params["values"].clone()
}

pub fn for_update(value: &Value) -> Result<(String, String, Value, Vec<Value>), ProviderFailure> {
    Ok(update_validation_result(update_params(value)?))
}

fn update_params(value: &Value) -> Result<&Map<String, Value>, ProviderFailure> {
    let params = required_object(value, "settings.update")?;
    ensure_update_params(params)?;
    Ok(params)
}

fn ensure_update_params(params: &Map<String, Value>) -> Result<(), ProviderFailure> {
    if params.len() != 3
        || params.get("id").and_then(Value::as_str).is_none()
        || params.get("version").and_then(Value::as_str).is_none()
        || !params.get("values").is_some_and(Value::is_object)
    {
        return Err(invalid_params("settings.update"));
    }
    Ok(())
}

fn update_validation_result(params: &Map<String, Value>) -> (String, String, Value, Vec<Value>) {
    let values = update_values(params);
    (
        update_id(params),
        update_version(params),
        values.clone(),
        validate_values(&values),
    )
}

fn update_id(params: &Map<String, Value>) -> String {
    params["id"].as_str().unwrap().to_string()
}

fn update_version(params: &Map<String, Value>) -> String {
    params["version"].as_str().unwrap().to_string()
}

fn update_values(params: &Map<String, Value>) -> Value {
    params["values"].clone()
}

pub fn for_migrate(value: &Value) -> Result<(bool, Value), ProviderFailure> {
    let params = required_object(value, "settings.migrate")?;
    if params.len() == 2
        && params.get("dry_run").and_then(Value::as_bool).is_some()
        && params.get("legacy").is_some_and(Value::is_object)
    {
        return Ok((
            params["dry_run"].as_bool().unwrap(),
            params["legacy"].clone(),
        ));
    }
    Err(invalid_params("settings.migrate"))
}

fn validate_params(value: &Value) -> Result<&Value, ProviderFailure> {
    let params = required_object(value, "settings.validate")?;
    if params.len() == 1 && params.contains_key("values") && params["values"].is_object() {
        return Ok(&params["values"]);
    }
    Err(invalid_params("settings.validate"))
}

fn validate_response(diagnostics: Vec<Value>) -> Value {
    let valid = validation_valid(&diagnostics);
    format_validate_response(valid, diagnostics)
}

fn validation_valid(diagnostics: &[Value]) -> bool {
    diagnostics.is_empty()
}

fn format_validate_response(valid: bool, diagnostics: Vec<Value>) -> Value {
    json!({
        "valid": valid,
        "diagnostics": diagnostics
    })
}

pub fn validate_values(values: &Value) -> Vec<Value> {
    let Some(object) = values_object(values) else {
        return vec![settings_values_must_be_object_diagnostic()];
    };

    value_rule_diagnostics(object)
}

fn values_object(values: &Value) -> Option<&Map<String, Value>> {
    values.as_object()
}

fn value_rule_diagnostics(object: &Map<String, Value>) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    diagnostics.extend(string_fields_diagnostics(object));
    diagnostics.extend(args_diagnostics(object));
    diagnostics.extend(tool_restrictions_diagnostics(object));
    diagnostics
}

fn string_fields_diagnostics(object: &Map<String, Value>) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    for key in [
        "command",
        "quota_script",
        "auth_refresh_command",
        "setup_brain_model",
    ] {
        string_field(object, key, &mut diagnostics);
    }
    diagnostics
}

fn args_diagnostics(object: &Map<String, Value>) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    if let Some(args) = object.get("args") {
        validate_args(args, &mut diagnostics);
    }
    diagnostics
}

fn validate_args(value: &Value, diagnostics: &mut Vec<Value>) {
    match value.as_array() {
        Some(items) if items.iter().all(Value::is_string) => {}
        _ => diagnostics.push(args_must_be_strings_diagnostic()),
    }
}

fn tool_restrictions_diagnostics(object: &Map<String, Value>) -> Vec<Value> {
    let mut diagnostics = Vec::new();
    if let Some(restrictions) = object.get("tool_restrictions") {
        validate_tool_restrictions(restrictions, &mut diagnostics);
    }
    diagnostics
}

pub fn required_object<'a>(
    value: &'a Value,
    capability: &str,
) -> Result<&'a Map<String, Value>, ProviderFailure> {
    value.as_object().ok_or_else(|| invalid_params(capability))
}

pub fn invalid_params(capability: &str) -> ProviderFailure {
    ProviderFailure::invalid_request(
        format!("invalid_{}_params", capability.replace('.', "_")),
        format!("{capability} params do not match the settings contract"),
    )
}

pub fn diagnostic(severity: &str, path: &str, code: &str, message: &str) -> Value {
    json!({
        "severity": severity,
        "path": path,
        "code": code,
        "message": message
    })
}

fn string_field(object: &Map<String, Value>, key: &str, diagnostics: &mut Vec<Value>) {
    if object.get(key).is_some_and(|value| !value.is_string()) {
        diagnostics.push(string_field_diagnostic(key));
    }
}

fn validate_tool_restrictions(value: &Value, diagnostics: &mut Vec<Value>) {
    let Some(object) = value.as_object() else {
        diagnostics.push(tool_restrictions_must_be_object_diagnostic());
        return;
    };
    if object.get("kind").is_some_and(|kind| kind != "claude") {
        diagnostics.push(unsupported_tool_restrictions_kind_diagnostic());
    }
}

fn settings_values_must_be_object_diagnostic() -> Value {
    diagnostic(
        "error",
        "values",
        "settings_values_must_be_object",
        "settings values must be an object",
    )
}

fn string_field_diagnostic(key: &str) -> Value {
    diagnostic(
        "error",
        &format!("values.{key}"),
        "settings_field_must_be_string",
        &format!("{key} must be a string"),
    )
}

fn args_must_be_strings_diagnostic() -> Value {
    diagnostic(
        "error",
        "values.args",
        "settings_args_must_be_strings",
        "args must be an array of strings",
    )
}

fn tool_restrictions_must_be_object_diagnostic() -> Value {
    diagnostic(
        "error",
        "values.tool_restrictions",
        "tool_restrictions_must_be_object",
        "tool_restrictions must be an object",
    )
}

fn unsupported_tool_restrictions_kind_diagnostic() -> Value {
    diagnostic(
        "error",
        "values.tool_restrictions.kind",
        "unsupported_tool_restrictions_kind",
        "only claude tool restrictions are supported",
    )
}
