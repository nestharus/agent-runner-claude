// declared_role: formatter, mapper, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    validate_params(request)?;
    Ok(models_response())
}

fn validate_params(request: &RequestEnvelope) -> Result<(), ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)?;
    Ok(())
}

fn models_response() -> Value {
    json!({
        "provider_version": "unknown",
        "models": [
            model("claude-sonnet-4-6", "Claude Sonnet 4.6"),
            model("claude-opus-4-5", "Claude Opus 4.5"),
            model("claude-haiku-4-5", "Claude Haiku 4.5")
        ],
        "warnings": [
            "Model aliases are hardcoded; availability is not CLI-probed by discovery.models."
        ]
    })
}

fn model(alias: &str, display_name: &str) -> Value {
    json!({
        "id": alias,
        "alias": alias,
        "display_name": display_name,
        "provider": "claude",
        "source": "hardcoded-known-alias",
        "availability": "not_probed"
    })
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_discovery_models_params",
        "discovery.models params do not match the discovery contract",
    )
}
