// declared_role: orchestration, validator, predicate, mapper, formatter

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let source = validate_source_request(request)?;
    let has_script = has_quota_script(source.context);
    let has_cache = has_quota_cache(source.context);
    let freshness = source_freshness(has_script, has_cache);

    Ok(source_result(
        source.settings_id,
        has_source(has_script, has_cache),
        freshness,
    ))
}

struct SourceRequest<'a> {
    settings_id: &'a str,
    context: &'a Value,
}

fn validate_source_request(
    request: &RequestEnvelope,
) -> Result<SourceRequest<'_>, ProviderFailure> {
    let params = request.params.as_object().ok_or_else(invalid_params)?;
    let settings_id = params
        .get("settings_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_params)?;
    let context = params.get("context").unwrap_or(&Value::Null);

    Ok(SourceRequest {
        settings_id,
        context,
    })
}

fn has_quota_script(context: &Value) -> bool {
    context
        .get("quota_script")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .is_some()
}

fn has_quota_cache(context: &Value) -> bool {
    context.get("quota_cache").is_some_and(Value::is_object)
}

fn has_source(has_script: bool, has_cache: bool) -> bool {
    has_script || has_cache
}

fn source_freshness(has_script: bool, has_cache: bool) -> &'static str {
    if has_cache {
        "fresh_cache"
    } else if has_script {
        "probe_available"
    } else {
        "unavailable"
    }
}

fn source_result(settings_id: &str, has_source: bool, freshness: &str) -> Value {
    let mut result = serde_json::Map::new();
    result.insert("has_source".to_string(), json!(has_source));
    result.insert("freshness".to_string(), json!(freshness));
    if has_source {
        result.insert("source_id".to_string(), json!(source_id(settings_id)));
    }

    Value::Object(result)
}

fn source_id(settings_id: &str) -> String {
    format!("claude:{settings_id}:quota")
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_quota_source_params",
        "quota.source params do not match the quota contract",
    )
}
