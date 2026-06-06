// declared_role: orchestration, accessor, parser, validator, formatter
// adapter_declarations:
//   - component: src/envelope/
//     role: adapter
//     Translates:
//       - contract/v1/common.schema.json#/$defs/RequestEnvelope
//       - contract/v1/common.schema.json#/$defs/SuccessResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorObject

use std::io::Read;

use serde_json::Value;

use super::error::ProviderFailure;
use super::CONTRACT;

#[derive(Debug, Clone)]
pub struct RequestEnvelope {
    pub contract: String,
    pub request_id: String,
    pub provider_instance_id: Option<String>,
    pub host: Value,
    pub params: Value,
}

struct RequestEnvelopeParts {
    contract: String,
    request_id: String,
    provider_instance_id: Option<String>,
    host: Value,
    params: Value,
}

pub fn decode_request<R: Read>(mut reader: R) -> Result<RequestEnvelope, ProviderFailure> {
    let bytes = read_request_bytes(&mut reader)?;
    let text = parse_utf8_request(&bytes)?;
    let value = parse_json_request(text)?;
    parse_request_value(value)
}

fn read_request_bytes<R: Read>(reader: &mut R) -> Result<Vec<u8>, ProviderFailure> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(stdin_read_failed)?;
    Ok(bytes)
}

fn stdin_read_failed(error: std::io::Error) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "stdin_read_failed",
        format!("failed to read request stdin: {error}"),
    )
}

fn parse_utf8_request(bytes: &[u8]) -> Result<&str, ProviderFailure> {
    std::str::from_utf8(bytes).map_err(|_| {
        ProviderFailure::invalid_request("invalid_utf8", "request envelope must be UTF-8 JSON")
    })
}

fn parse_json_request(text: &str) -> Result<Value, ProviderFailure> {
    parse_json_value(text)
        .map_err(|error| invalid_json_request(parse_error_request_id(text), error))
}

fn parse_json_value(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text)
}

fn parse_error_request_id(text: &str) -> String {
    request_id_from_raw_json(text).unwrap_or_else(fallback_request_id)
}

fn invalid_json_request(request_id: String, error: serde_json::Error) -> ProviderFailure {
    ProviderFailure::invalid_request_with_request_id(
        request_id,
        "invalid_json",
        format!("request envelope must be valid JSON: {error}"),
    )
}

fn parse_request_value(value: Value) -> Result<RequestEnvelope, ProviderFailure> {
    let object = request_object(&value)?;
    let request_id = request_id_for_errors(object);
    reject_unsupported_fields(object, &request_id)?;
    let parts = request_envelope_parts(object, &request_id)?;
    validate_contract(&parts.contract, &parts.request_id)?;
    validate_host(&parts.host, &parts.request_id)?;
    Ok(request_envelope_from_parts(parts))
}

fn request_object(value: &Value) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    value.as_object().ok_or_else(|| {
        ProviderFailure::invalid_request(
            "invalid_envelope",
            "request envelope must be a JSON object",
        )
    })
}

fn request_id_for_errors(object: &serde_json::Map<String, Value>) -> String {
    object
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(fallback_request_id)
}

fn reject_unsupported_fields(
    object: &serde_json::Map<String, Value>,
    request_id: &str,
) -> Result<(), ProviderFailure> {
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "contract" | "request_id" | "provider_instance_id" | "host" | "params"
        ) {
            return Err(ProviderFailure::invalid_request_with_request_id(
                request_id.to_string(),
                "invalid_envelope",
                format!("unsupported request envelope field: {key}"),
            ));
        }
    }
    Ok(())
}

fn request_envelope_parts(
    object: &serde_json::Map<String, Value>,
    request_id: &str,
) -> Result<RequestEnvelopeParts, ProviderFailure> {
    let contract = required_string(object, "contract", request_id)?;
    let request_id_field = required_string(object, "request_id", request_id)?;
    let host = cloned_value(required_value(object, "host", &request_id_field)?);
    let params = cloned_value(required_value(object, "params", &request_id_field)?);
    let provider_instance_id = provider_instance_id(object);

    Ok(RequestEnvelopeParts {
        contract,
        request_id: request_id_field,
        provider_instance_id,
        host,
        params,
    })
}

fn required_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    request_id: &str,
) -> Result<&'a Value, ProviderFailure> {
    field_value(object, key).ok_or_else(|| missing_envelope_value(request_id, key))
}

fn field_value<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key)
}

fn cloned_value(value: &Value) -> Value {
    value.clone()
}

fn missing_envelope_value(request_id: &str, key: &str) -> ProviderFailure {
    ProviderFailure::invalid_request_with_request_id(
        request_id.to_string(),
        format!("missing_{key}"),
        format!("request envelope missing {key}"),
    )
}

fn provider_instance_id(object: &serde_json::Map<String, Value>) -> Option<String> {
    provider_instance_id_value(object).map(str::to_string)
}

fn provider_instance_id_value(object: &serde_json::Map<String, Value>) -> Option<&str> {
    object.get("provider_instance_id").and_then(Value::as_str)
}

fn validate_contract(contract: &str, request_id: &str) -> Result<(), ProviderFailure> {
    if contract == CONTRACT {
        Ok(())
    } else {
        Err(ProviderFailure::unsupported(
            "unsupported_contract",
            format!("unsupported provider contract: {contract}"),
        )
        .with_request_id(request_id.to_string()))
    }
}

fn request_envelope_from_parts(parts: RequestEnvelopeParts) -> RequestEnvelope {
    RequestEnvelope {
        contract: parts.contract,
        request_id: parts.request_id,
        provider_instance_id: parts.provider_instance_id,
        host: parts.host,
        params: parts.params,
    }
}

fn required_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    request_id: &str,
) -> Result<String, ProviderFailure> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderFailure::invalid_request_with_request_id(
                request_id.to_string(),
                format!("missing_{key}"),
                format!("request envelope missing string field: {key}"),
            )
        })
}

fn validate_host(host: &Value, request_id: &str) -> Result<(), ProviderFailure> {
    let Some(object) = host.as_object() else {
        return Err(ProviderFailure::invalid_request_with_request_id(
            request_id.to_string(),
            "invalid_host",
            "host must be a JSON object",
        ));
    };
    if object
        .get("app")
        .and_then(Value::as_str)
        .filter(|app| !app.is_empty())
        .is_none()
    {
        return Err(ProviderFailure::invalid_request_with_request_id(
            request_id.to_string(),
            "invalid_host",
            "host.app is required",
        ));
    }
    Ok(())
}

fn request_id_from_raw_json(text: &str) -> Option<String> {
    raw_json_value(text)
        .as_ref()
        .and_then(raw_json_request_id)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn raw_json_value(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok()
}

fn raw_json_request_id(value: &Value) -> Option<&str> {
    value.get("request_id").and_then(Value::as_str)
}

fn fallback_request_id() -> String {
    "unknown-request".to_string()
}
