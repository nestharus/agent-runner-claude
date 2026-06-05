// declared_role: accessor, formatter, mapper

use serde_json::{json, Value};

pub fn materialize_plan(params: &Value, artifacts: &[Value]) -> Value {
    let source_provider = string(params, "source_provider", "claude");
    let target_provider = string(params, "target_provider", "claude");
    let source_session_id = string(params, "source_session_id", "unknown-source-session");
    let target_session_id = string(params, "target_session_id", "unknown-target-session");

    json!({
        "schema_version": 1,
        "operation": "rotation.materialize",
        "chain_id": string(params, "chain_id", "rotation-chain"),
        "source_provider": source_provider,
        "target_provider": target_provider,
        "source_session_id": source_session_id,
        "target_session_id": target_session_id,
        "transition_reason": transition_reason(params),
        "segments": [
            { "provider": source_provider, "session_id": source_session_id },
            { "provider": target_provider, "session_id": target_session_id }
        ],
        "artifacts": artifacts.iter().map(|artifact| json!({
            "kind": "file",
            "path": artifact.get("path").and_then(Value::as_str).unwrap_or("provider-artifact"),
            "sha256": artifact.get("sha256").and_then(Value::as_str).unwrap_or("")
        })).collect::<Vec<_>>()
    })
}

fn transition_reason(params: &Value) -> &'static str {
    match params.get("transition_reason").and_then(Value::as_str) {
        Some("manual") => "manual",
        Some("quota_threshold") => "quota_threshold",
        Some("exhausted") => "exhausted",
        _ => "manual",
    }
}

fn string(params: &Value, key: &str, default: &str) -> String {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}
