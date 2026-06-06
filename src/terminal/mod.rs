// declared_role: orchestration, validator, parser, mapper, formatter

pub mod classifier;
pub mod evidence;
pub mod params;
pub mod status;

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(_subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    classify(request)
}

fn classify(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = validate_request(params::params_value(&request.params))?;
    let stdout = decode_request_field(params.stdout_base64, "stdout_base64")?;
    let stderr = decode_request_field(params.stderr_base64, "stderr_base64")?;
    let status = status::parse_status(params.status).ok_or_else(invalid_params)?;
    let kind = classifier::classify(status, &stdout, &stderr);
    Ok(signal_response(
        kind,
        params.observed_at_unix_ms.clone(),
        &stdout,
        &stderr,
    ))
}

struct TerminalRequest<'a> {
    stdout_base64: &'a str,
    stderr_base64: &'a str,
    observed_at_unix_ms: &'a Value,
    status: &'a Value,
}

fn validate_request(value: &Value) -> Result<TerminalRequest<'_>, ProviderFailure> {
    let params = value.as_object().ok_or_else(invalid_params)?;
    if params.len() != 4
        || params
            .get("stdout_base64")
            .and_then(Value::as_str)
            .is_none()
        || params
            .get("stderr_base64")
            .and_then(Value::as_str)
            .is_none()
        || params
            .get("observed_at_unix_ms")
            .and_then(Value::as_u64)
            .is_none()
        || !params.get("status").is_some_and(Value::is_object)
    {
        return Err(invalid_params());
    }

    Ok(TerminalRequest {
        stdout_base64: params["stdout_base64"].as_str().unwrap(),
        stderr_base64: params["stderr_base64"].as_str().unwrap(),
        observed_at_unix_ms: &params["observed_at_unix_ms"],
        status: &params["status"],
    })
}

fn signal_response(kind: &str, observed_at_unix_ms: Value, stdout: &[u8], stderr: &[u8]) -> Value {
    let mut signal = serde_json::Map::new();
    signal.insert("kind".to_string(), json!(kind));
    signal.insert("observed_at_unix_ms".to_string(), observed_at_unix_ms);
    if let Some(evidence) = evidence::evidence(stdout, stderr) {
        signal.insert("evidence".to_string(), json!(evidence));
    }
    json!({ "terminal_signal": Value::Object(signal) })
}

fn decode_request_field(value: &str, field: &str) -> Result<Vec<u8>, ProviderFailure> {
    decode_field(value).map_err(|error| invalid_base64_field(field, error))
}

fn decode_field(value: &str) -> Result<Vec<u8>, String> {
    crate::encoding::decode_base64(value)
}

fn invalid_base64_field(field: &str, error: impl std::fmt::Display) -> ProviderFailure {
    ProviderFailure::invalid_request(
        format!("invalid_{field}"),
        format!("{field} must be valid base64: {error}"),
    )
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_terminal_classify_params",
        "terminal.classify params do not match the terminal contract",
    )
}
