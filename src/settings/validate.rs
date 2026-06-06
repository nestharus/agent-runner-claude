// declared_role: accessor, formatter, mapper, orchestration, validator

use jsonschema::{Draft, JSONSchema};
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
    if values_object(values).is_none() {
        return vec![settings_values_must_be_object_diagnostic()];
    }
    provider_settings_schema_diagnostics(values)
}

fn values_object(values: &Value) -> Option<&Map<String, Value>> {
    values.as_object()
}

fn provider_settings_schema_diagnostics(values: &Value) -> Vec<Value> {
    let compiled = compiled_provider_settings_schema();
    compiled
        .validate(values)
        .map(|_| Vec::new())
        .unwrap_or_else(|errors| errors.map(settings_schema_diagnostic).collect())
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

fn compiled_provider_settings_schema() -> JSONSchema {
    JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(&crate::settings_schema::settings_schema())
        .expect("claude.settings/v1 schema must compile")
}

fn settings_schema_diagnostic(error: jsonschema::error::ValidationError<'_>) -> Value {
    diagnostic(
        "error",
        &settings_schema_diagnostic_path(&error.instance_path.to_string()),
        "invalid_settings_value",
        &format!("settings value does not satisfy claude.settings/v1: {error}"),
    )
}

fn settings_schema_diagnostic_path(instance_path: &str) -> String {
    if instance_path.is_empty() {
        "values".to_string()
    } else {
        format!("values{instance_path}")
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
