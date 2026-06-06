// declared_role: accessor, formatter, orchestration, predicate, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;
use crate::envelope::CONTRACT;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    validate_empty_params(request)?;
    Ok(format_description())
}

fn validate_empty_params(request: &RequestEnvelope) -> Result<(), ProviderFailure> {
    let Some(params) = request.params.as_object() else {
        return Err(ProviderFailure::invalid_request(
            "invalid_describe_params",
            "describe params must be an object",
        ));
    };
    if !params.is_empty() {
        return Err(ProviderFailure::invalid_request(
            "invalid_describe_params",
            "describe params must be empty",
        ));
    }
    Ok(())
}

fn format_description() -> Value {
    json!({
        "provider_id": "claude",
        "display_name": "Claude Code",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "settings_schema_id": crate::settings_schema::SCHEMA_ID,
        "capabilities": {
            "launch": true,
            "policy": true,
            "quota": true,
            "session": true,
            "terminal": true,
            "rotation": true,
            "discovery": true,
            "settings": true,
            "setup_brain": true,
            "setup": true,
            "migration": true
        },
        "concurrency": {
            "process_model": "one_shot_cli",
            "state_serialization": "provider_advisory_locks",
            "launch_streams": "one_per_process"
        }
    })
}
