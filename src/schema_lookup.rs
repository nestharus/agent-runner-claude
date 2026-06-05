// declared_role: accessor, filter, formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let schema_id = validate_schema_id(request)?;
    Ok(format_schema_result(schema_id))
}

fn validate_schema_id(request: &RequestEnvelope) -> Result<&str, ProviderFailure> {
    let Some(params) = request.params.as_object() else {
        return Err(ProviderFailure::invalid_request(
            "invalid_schema_params",
            "schema params must be an object",
        ));
    };
    if params.len() != 1 {
        return Err(ProviderFailure::invalid_request(
            "invalid_schema_params",
            "schema params must contain only schema_id",
        ));
    }
    let Some(schema_id) = params
        .get("schema_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        return Err(ProviderFailure::invalid_request(
            "invalid_schema_id",
            "schema_id is required",
        ));
    };
    if schema_id != crate::settings_schema::SCHEMA_ID {
        return Err(ProviderFailure::unsupported(
            "unknown_schema_id",
            format!("unsupported schema id: {schema_id}"),
        ));
    }
    Ok(schema_id)
}

fn format_schema_result(schema_id: &str) -> Value {
    json!({
        "schema_id": schema_id,
        "schema": crate::settings_schema::settings_schema(),
        "ui": {}
    })
}
