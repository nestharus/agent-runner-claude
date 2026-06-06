// declared_role: accessor, formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = install_params(request)?;
    Ok(install_plan_response(
        install_target(params),
        install_channel(params),
    ))
}

fn install_params(
    request: &RequestEnvelope,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    request.params.as_object().ok_or_else(invalid_params)
}

fn install_target(params: &serde_json::Map<String, Value>) -> &str {
    params
        .get("install_target")
        .and_then(Value::as_str)
        .unwrap_or("~/.local/bin/claude")
}

fn install_channel(params: &serde_json::Map<String, Value>) -> &str {
    params
        .get("channel")
        .and_then(Value::as_str)
        .unwrap_or("stable")
}

fn install_plan_response(target: &str, channel: &str) -> Value {
    json!({
        "steps": [
            {
                "kind": "check_binary",
                "description": "Check whether Claude Code is already installed on PATH",
                "target": "claude"
            },
            {
                "kind": "install_cli",
                "description": format!("Install Claude Code CLI from the {channel} channel"),
                "target": target,
                "mutates": false
            },
            {
                "kind": "authenticate",
                "description": "Run claude auth login outside this plan if authentication is missing",
                "mutates": false
            }
        ]
    })
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_setup_install_plan_params",
        "setup.install_plan params do not match the setup contract",
    )
}
