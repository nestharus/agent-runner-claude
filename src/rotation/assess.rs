// declared_role: accessor, formatter, mapper, orchestration, predicate, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = assess_params(request)?;
    let terminal_signal = terminal_signal(params);
    let transition_reason = transition_reason(params);
    Ok(assessment_response(
        provider_pair_allowed(params),
        assessment_score(terminal_signal, remaining_percent(params)),
        transition_reason,
        terminal_signal,
    ))
}

fn assess_params(
    request: &RequestEnvelope,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)
}

fn remaining_percent(params: &serde_json::Map<String, Value>) -> Option<i64> {
    params
        .get("quota")
        .and_then(|quota| quota.get("remaining_percent"))
        .and_then(Value::as_i64)
}

fn terminal_signal(params: &serde_json::Map<String, Value>) -> &str {
    params
        .get("quota")
        .and_then(|quota| quota.get("terminal_signal"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn transition_reason(params: &serde_json::Map<String, Value>) -> &str {
    params
        .get("transition_reason")
        .and_then(Value::as_str)
        .unwrap_or("manual")
}

fn assessment_score(terminal_signal: &str, remaining: Option<i64>) -> i64 {
    match (terminal_signal, remaining) {
        ("quota_exhausted_inband" | "rate_limited", _) => 95,
        (_, Some(value)) if value <= 5 => 85,
        (_, Some(value)) if value <= 20 => 55,
        _ => 25,
    }
}

fn provider_pair_allowed(params: &serde_json::Map<String, Value>) -> bool {
    params
        .get("source_provider")
        .and_then(Value::as_str)
        .is_some_and(|provider| provider == "claude")
        && params
            .get("target_provider")
            .and_then(Value::as_str)
            .is_some_and(|provider| provider == "claude")
}

fn assessment_response(
    allowed: bool,
    score: i64,
    transition_reason: &str,
    terminal_signal: &str,
) -> Value {
    json!({
        "allowed": allowed,
        "score": score,
        "reason": format!("rotation assessment for {transition_reason} with signal {terminal_signal}"),
        "requirements": [
            "host must apply returned host_state_plan separately",
            "provider-owned artifacts only",
            "host database and journal are not mutated by assess"
        ]
    })
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_rotation_assess_params",
        "rotation.assess params do not match the rotation contract",
    )
}
