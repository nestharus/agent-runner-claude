// declared_role: accessor, formatter, mapper, orchestration, parser, predicate

use std::fs;

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::types::{optional_string, parse_base_params, required_string};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let base = parse_base_params(request)?;
    let _ = base.settings_id;
    let strategy =
        optional_string(&request.params, "strategy").unwrap_or_else(|| "none".to_string());
    match strategy.as_str() {
        "none" => Ok(none_response()),
        "forced_flag_readback" => forced_flag_readback(request),
        "stdout_json_event" => stdout_json_event(request),
        "start_known" => start_known(request),
        _ => Err(unsupported_capture_strategy(&strategy)),
    }
}

fn unsupported_capture_strategy(strategy: &str) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "unsupported_capture_strategy",
        format!("unsupported session capture strategy: {strategy}"),
    )
}

fn forced_flag_readback(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let transcript_path = confined_evidence_path(request)?;
    let transcript = read_evidence_file(&transcript_path)?;
    let provider_session_id = extract_evidence_session_id(&transcript);
    Ok(forced_flag_response(
        provider_session_id,
        transcript_path.display().to_string(),
    ))
}

fn stdout_json_event(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let stdout_base64 = required_string(&request.params, "stdout_base64")?;
    let stdout = crate::encoding::decode_base64(&stdout_base64).map_err(|error| {
        ProviderFailure::invalid_request(
            "invalid_stdout_base64",
            format!("stdout_base64 must be valid base64: {error}"),
        )
    })?;
    let provider_session_id = extract_stdout_session_id(&stdout);
    Ok(stdout_event_response(provider_session_id))
}

fn start_known(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let provider_session_id = required_string(&request.params, "provider_session_id")?;
    Ok(start_known_response(provider_session_id))
}

fn none_response() -> Value {
    json!({
        "provider_session_id": Value::Null,
        "state": {
            "kind": "not_captured",
            "strategy": "none"
        },
        "artifacts": [],
    })
}

fn forced_flag_response(provider_session_id: Option<String>, transcript_path: String) -> Value {
    json!({
        "provider_session_id": provider_session_id.clone(),
        "state": {
            "source": "forced_flag_readback",
            "provider_session_id": provider_session_id,
        },
        "artifacts": [{ "kind": "transcript", "path": transcript_path }],
    })
}

fn stdout_event_response(provider_session_id: Option<String>) -> Value {
    json!({
        "provider_session_id": provider_session_id,
        "state": {
            "source": "stdout_json_event",
            "provider_session_id": provider_session_id,
        },
        "artifacts": [{ "kind": "stdout_event" }],
    })
}

fn start_known_response(provider_session_id: String) -> Value {
    json!({
        "provider_session_id": provider_session_id,
        "state": {
            "source": "start_known",
            "provider_session_id": provider_session_id,
        },
        "artifacts": [],
    })
}

fn extract_stdout_session_id(stdout: &[u8]) -> Option<String> {
    stdout_json_values(stdout)
        .into_iter()
        .find_map(|value| stdout_capture_session_id(&value))
}

fn stdout_json_values(stdout: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().filter_map(parse_json_line).collect()
}

fn stdout_capture_session_id(value: &Value) -> Option<String> {
    if is_stdout_capture_event(value) {
        json_session_id(value).map(str::to_string)
    } else {
        None
    }
}

fn confined_evidence_path(
    request: &RequestEnvelope,
) -> Result<std::path::PathBuf, ProviderFailure> {
    let raw_path = required_string(&request.params, "transcript_path")?;
    let home = super::types::host_home(request)?;
    super::storage::confined_transcript_path(&home, &raw_path).map_err(|error| {
        super::types::conflict(
            "session_capture_transcript_path_outside_provider_root",
            format!("session.capture transcript evidence path is not provider-owned: {error}"),
        )
    })
}

fn read_evidence_file(path: &std::path::Path) -> Result<Vec<u8>, ProviderFailure> {
    fs::read(path).map_err(|error| {
        ProviderFailure::invalid_request(
            "session_capture_evidence_unavailable",
            format!("session capture evidence could not be read: {error}"),
        )
    })
}

fn extract_evidence_session_id(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        if let Some(session_id) = json_evidence_session_id(line) {
            return Some(session_id);
        }
        if let Some(session_id) = marker_session_id(line) {
            return Some(session_id.to_string());
        }
    }
    None
}

fn json_evidence_session_id(line: &str) -> Option<String> {
    let value = parse_json_line(line)?;
    if !is_evidence_capture_event(&value) {
        return None;
    }
    json_session_id(&value).map(str::to_string)
}

fn parse_json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line.trim()).ok()
}

fn json_session_id(value: &Value) -> Option<&str> {
    value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
}

fn is_stdout_capture_event(value: &Value) -> bool {
    string_field_equals(value, "type", "system")
        && string_field_equals(value, "subtype", "init")
        && json_session_id(value).is_some()
}

fn is_evidence_capture_event(value: &Value) -> bool {
    string_field_equals(value, "type", "claude_session_capture_event")
        || string_field_equals(value, "subtype", "claude_session_capture_event")
        || string_field_equals(value, "event", "claude_session_capture_event")
}

fn string_field_equals(value: &Value, key: &str, expected: &str) -> bool {
    value.get(key).and_then(Value::as_str) == Some(expected)
}

fn marker_session_id(line: &str) -> Option<&str> {
    line.trim()
        .strip_prefix("claude_session_capture_event=")
        .filter(|value| !value.is_empty())
}
