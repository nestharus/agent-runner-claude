// declared_role: accessor, formatter, mapper, orchestration, parser, validator
// adapter_declarations:
//   - component: src/setup/brain_turn.rs
//     role: adapter
//     Translates:
//       - contract/v1/setup.schema.json#/$defs/SetupBrainTurnRequest
//       - contract/v1/setup.schema.json#/$defs/SetupBrainTurnResult
//       - src/setup/brain_cli.rs claude -p argv adapter seam
//       - src/settings/store.rs setup brain model selection seam
//       - src/external/shell.rs command stdout/stderr/exit seam

use std::collections::BTreeMap;
use std::io;

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = request.params.as_object().ok_or_else(invalid_params)?;
    let prompt = prompt(params)?;
    let resume = resume(params);
    let model = model_from_params(request, params)?;
    let schema = output_schema();
    let output =
        super::brain_cli::run_setup_brain(&model, &schema, resume, prompt, env_map(&request.host))
            .map_err(setup_brain_spawn_failed)?;
    require_successful_output(&output)?;
    let message = stdout_message(&output.stdout)?;
    let conversation_id = conversation_id(&output.stderr);

    Ok(brain_turn_response(&conversation_id, message, &model))
}

fn setup_brain_spawn_failed(error: io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "setup_brain_spawn_failed",
        format!("failed to run claude setup brain: {error}"),
        true,
    )
}

fn prompt(params: &serde_json::Map<String, Value>) -> Result<&str, ProviderFailure> {
    params
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_params)
}

fn resume(params: &serde_json::Map<String, Value>) -> Option<&str> {
    params
        .get("resume")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn output_schema() -> String {
    serde_json::to_string(&json!({
        "type": "object",
        "additionalProperties": true
    }))
    .expect("static JSON schema serializes")
}

fn require_successful_output(
    output: &crate::external::shell::CommandOutput,
) -> Result<(), ProviderFailure> {
    if output.timed_out {
        return Err(ProviderFailure::new(
            ErrorCategory::Timeout,
            "setup_brain_timeout",
            "claude setup brain timed out",
            true,
        ));
    }
    if output.status_code == Some(0) {
        Ok(())
    } else {
        Err(ProviderFailure::new(
            ErrorCategory::Unavailable,
            "setup_brain_failed",
            "claude setup brain command failed",
            true,
        ))
    }
}

fn stdout_message(stdout: &[u8]) -> Result<Value, ProviderFailure> {
    serde_json::from_slice(stdout).map_err(invalid_stdout_json)
}

fn invalid_stdout_json(error: serde_json::Error) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "setup_brain_invalid_stdout_json",
        format!("claude setup brain stdout was not JSON: {error}"),
    )
}

fn conversation_id(stderr: &[u8]) -> String {
    extract_session_id(&String::from_utf8_lossy(stderr))
        .unwrap_or_else(|| "unknown-session".to_string())
}

fn brain_turn_response(conversation_id: &str, message: Value, model: &str) -> Value {
    json!({
        "conversation_id": conversation_id,
        "message": message,
        "markers": [
            { "name": "claude_session_id", "value": conversation_id },
            { "name": "setup_brain_model", "value": model }
        ]
    })
}

fn model_from_params(
    request: &RequestEnvelope,
    params: &serde_json::Map<String, Value>,
) -> Result<String, ProviderFailure> {
    if let Some(settings_id) = params.get("settings_id").and_then(Value::as_str) {
        if let Some(model) =
            crate::settings::store::setup_brain_model_for_host(&request.host, settings_id)?
        {
            return Ok(model);
        }
    }
    Ok(super::brain_cli::default_setup_brain_model().to_string())
}

fn extract_session_id(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        if let Some(session) = line.strip_prefix("Session: ") {
            return Some(session.trim().to_string());
        }
        if let Some(session) = line.strip_prefix("session_id: ") {
            return Some(session.trim().to_string());
        }
    }
    None
}

fn env_map(host: &Value) -> BTreeMap<String, String> {
    host.get("env")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_setup_brain_turn_params",
        "setup_brain.turn params do not match the setup contract",
    )
}
