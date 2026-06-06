// declared_role: accessor, filter, formatter, mapper, orchestration, parser, validator
// adapter_declarations:
//   - component: src/session/types.rs
//     role: adapter
//     Translates:
//       - contract/v1/session.schema.json#/$defs/SessionBaseParams

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

#[derive(Debug, Clone)]
pub struct SessionKey {
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub struct SessionParams {
    pub settings_id: String,
    pub session_id: Option<String>,
}

pub fn parse_base_params(request: &RequestEnvelope) -> Result<SessionParams, ProviderFailure> {
    let object = params_object(request)?;
    let settings_id = required_string(&request.params, "settings_id")?;
    Ok(session_params(
        settings_id,
        optional_string_from_object(object, "session_id"),
    ))
}

pub fn required_session_id(request: &RequestEnvelope) -> Result<String, ProviderFailure> {
    require_session_id(parse_base_params(request)?.session_id)
}

pub fn required_string(params: &Value, key: &str) -> Result<String, ProviderFailure> {
    require_string_value(non_empty_field(params, key), key)
}

pub fn optional_string(params: &Value, key: &str) -> Option<String> {
    non_empty_field(params, key).map(str::to_string)
}

pub fn optional_path(params: &Value, key: &str) -> Option<PathBuf> {
    optional_string(params, key).map(PathBuf::from)
}

pub fn host_home(request: &RequestEnvelope) -> Result<PathBuf, ProviderFailure> {
    request_home(request).ok_or_else(missing_host_home)
}

fn missing_host_home() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "missing_host_home",
        "host.env.HOME is required for Claude session path resolution",
    )
}

fn params_object(request: &RequestEnvelope) -> Result<&Map<String, Value>, ProviderFailure> {
    request
        .params
        .as_object()
        .ok_or_else(invalid_session_params)
}

fn invalid_session_params() -> ProviderFailure {
    ProviderFailure::invalid_request("invalid_session_params", "session params must be an object")
}

fn session_params(settings_id: String, session_id: Option<String>) -> SessionParams {
    SessionParams {
        settings_id,
        session_id,
    }
}

fn optional_string_from_object(object: &Map<String, Value>, key: &str) -> Option<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn require_session_id(session_id: Option<String>) -> Result<String, ProviderFailure> {
    session_id.ok_or_else(missing_session_id)
}

fn missing_session_id() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "missing_session_id",
        "session params require non-empty session_id",
    )
}

fn non_empty_field<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn require_string_value(value: Option<&str>, key: &str) -> Result<String, ProviderFailure> {
    value.map(str::to_string).ok_or_else(|| missing_string(key))
}

fn missing_string(key: &str) -> ProviderFailure {
    ProviderFailure::invalid_request(
        format!("missing_{key}"),
        format!("session params require non-empty {key}"),
    )
}

fn request_home(request: &RequestEnvelope) -> Option<PathBuf> {
    request
        .host
        .get("env")
        .and_then(|env| env.get("HOME"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn conflict(code: impl Into<String>, message: impl Into<String>) -> ProviderFailure {
    ProviderFailure::new(ErrorCategory::Conflict, code, message, false)
}

pub fn failed(code: impl Into<String>, message: impl Into<String>) -> ProviderFailure {
    ProviderFailure::new(ErrorCategory::Failed, code, message, false)
}

pub fn invalid_request(code: impl Into<String>, message: impl Into<String>) -> ProviderFailure {
    ProviderFailure::invalid_request(code, message)
}
