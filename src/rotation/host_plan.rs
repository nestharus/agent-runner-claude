// declared_role: accessor, formatter, mapper

use serde_json::{json, Value};

struct PlanFacts {
    chain_id: String,
    source_provider: String,
    target_provider: String,
    source_session_id: String,
    target_session_id: String,
    transition_reason: &'static str,
}

struct ArtifactSummary {
    path: String,
    sha256: String,
}

pub fn materialize_plan(params: &Value, artifacts: &[Value]) -> Value {
    format_materialized_plan(plan_facts(params), artifact_summaries(artifacts))
}

fn plan_facts(params: &Value) -> PlanFacts {
    PlanFacts {
        chain_id: string(params, "chain_id", "rotation-chain"),
        source_provider: string(params, "source_provider", "claude"),
        target_provider: string(params, "target_provider", "claude"),
        source_session_id: string(params, "source_session_id", "unknown-source-session"),
        target_session_id: string(params, "target_session_id", "unknown-target-session"),
        transition_reason: transition_reason(params),
    }
}

fn artifact_summaries(artifacts: &[Value]) -> Vec<ArtifactSummary> {
    artifacts.iter().map(artifact_summary).collect()
}

fn artifact_summary(artifact: &Value) -> ArtifactSummary {
    ArtifactSummary {
        path: artifact_string(artifact, "path", "provider-artifact"),
        sha256: artifact_string(artifact, "sha256", ""),
    }
}

fn artifact_string(artifact: &Value, key: &str, default: &str) -> String {
    artifact
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn format_materialized_plan(facts: PlanFacts, artifacts: Vec<ArtifactSummary>) -> Value {
    json!({
        "schema_version": 1,
        "operation": "rotation.materialize",
        "chain_id": facts.chain_id,
        "source_provider": facts.source_provider.clone(),
        "target_provider": facts.target_provider.clone(),
        "source_session_id": facts.source_session_id.clone(),
        "target_session_id": facts.target_session_id.clone(),
        "transition_reason": facts.transition_reason,
        "segments": [
            { "provider": facts.source_provider, "session_id": facts.source_session_id },
            { "provider": facts.target_provider, "session_id": facts.target_session_id }
        ],
        "artifacts": artifacts.into_iter().map(format_artifact_summary).collect::<Vec<_>>()
    })
}

fn format_artifact_summary(summary: ArtifactSummary) -> Value {
    json!({
        "kind": "file",
        "path": summary.path,
        "sha256": summary.sha256
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
