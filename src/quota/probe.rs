// declared_role: orchestration, validator, predicate, formatter

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};
use crate::external::shell::CommandOutput;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let probe = validate_probe_request(request)?;
    let output = run_probe_script(probe.script)?;
    validate_probe_output(&output)?;

    let windows = super::parse::parse_windows(&output.stdout).map_err(quota_probe_parse_failed)?;
    validate_probe_windows(&windows, had_prior_windows(probe.context))?;

    Ok(probe_result(windows))
}

struct ProbeRequest<'a> {
    context: &'a serde_json::Map<String, Value>,
    script: &'a str,
}

fn validate_probe_request(request: &RequestEnvelope) -> Result<ProbeRequest<'_>, ProviderFailure> {
    let params = request.params.as_object().ok_or_else(invalid_params)?;
    params
        .get("settings_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_params)?;
    let context = params
        .get("context")
        .and_then(Value::as_object)
        .ok_or_else(invalid_params)?;
    let script = context
        .get("quota_script")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_params)?;

    Ok(ProbeRequest { context, script })
}

fn run_probe_script(script: &str) -> Result<CommandOutput, ProviderFailure> {
    super::scripts::run_quota_script(script).map_err(quota_probe_unavailable)
}

fn validate_probe_output(output: &CommandOutput) -> Result<(), ProviderFailure> {
    if output.timed_out {
        return Err(quota_probe_timeout());
    }

    if output.status_code == Some(0) {
        return Ok(());
    }

    Err(quota_probe_failed())
}

fn validate_probe_windows(windows: &[Value], had_prior: bool) -> Result<(), ProviderFailure> {
    if windows.is_empty() && had_prior {
        return Err(quota_probe_empty_after_prior_data());
    }

    Ok(())
}

fn had_prior_windows(context: &serde_json::Map<String, Value>) -> bool {
    context
        .get("prior_windows")
        .and_then(Value::as_array)
        .is_some_and(|windows| !windows.is_empty())
}

fn probe_result(windows: Vec<Value>) -> Value {
    json!({
        "available": true,
        "checked_at_unix_ms": crate::encoding::now_unix_ms(),
        "windows": windows,
    })
}

fn quota_probe_unavailable(error: std::io::Error) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "quota_probe_unavailable",
        format!("failed to run quota probe: {error}"),
        true,
    )
}

fn quota_probe_parse_failed(error: String) -> ProviderFailure {
    ProviderFailure::invalid_request("quota_probe_parse_failed", error)
}

fn quota_probe_timeout() -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Timeout,
        "quota_probe_timeout",
        "quota probe timed out",
        true,
    )
}

fn quota_probe_failed() -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "quota_probe_failed",
        "quota probe command failed",
        true,
    )
}

fn quota_probe_empty_after_prior_data() -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Unavailable,
        "quota_probe_empty_after_prior_data",
        "quota probe returned no windows after prior quota data was available; refresh is recommended",
        true,
    )
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_quota_probe_params",
        "quota.probe params do not match the quota contract",
    )
}
