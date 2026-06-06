// declared_role: accessor, filter, formatter, mapper, orchestration, parser, predicate, validator
// intrinsic_surface_declarations:
//   - component: src/policy/mod.rs
//     role: intrinsic-surface
//     Domain: policy_capability_module_index
//     Owns:
//       - policy capability submodule declaration set
//       - policy.evaluate request orchestration surface

pub mod argv;
pub mod diagnostics;
pub mod params;
pub mod prompt;
pub mod restrictions;

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(_subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    evaluate(request)
}

fn evaluate(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let policy = validate_request(params::params_value(&request.params))?;
    let provider_args = provider_args(policy.model).ok_or_else(invalid_params)?;
    let mut argv = argv::base_argv(policy.launch, &provider_args).ok_or_else(invalid_params)?;
    ensure_nonempty_argv(&argv)?;

    let prompt = prompt::project(policy.model, policy.launch).ok_or_else(invalid_params)?;
    let restrictions = restrictions::parse(policy.launch).ok_or_else(invalid_params)?;
    let diagnostics = diagnostics::policy_diagnostics(&argv, &restrictions, policy.proxy_mode);
    append_system_prompt_override(policy.launch, &mut argv);
    restrictions.append_argv(&mut argv);
    append_prompt_arg(&prompt, &mut argv);

    Ok(policy_response(argv, prompt, diagnostics))
}

fn ensure_nonempty_argv(argv: &[String]) -> Result<(), ProviderFailure> {
    if argv.is_empty() {
        return Err(invalid_params());
    }
    Ok(())
}

struct PolicyRequest<'a> {
    model: &'a Value,
    launch: &'a Value,
    proxy_mode: bool,
}

fn validate_request(value: &Value) -> Result<PolicyRequest<'_>, ProviderFailure> {
    let params = value.as_object().ok_or_else(invalid_params)?;
    if params.len() != 4
        || params.get("settings_id").and_then(Value::as_str).is_none()
        || params.get("mode").and_then(Value::as_str).is_none()
        || !params.get("model").is_some_and(Value::is_object)
        || !params.get("launch").is_some_and(Value::is_object)
    {
        return Err(invalid_params());
    }
    Ok(PolicyRequest {
        model: &params["model"],
        launch: &params["launch"],
        proxy_mode: proxy_mode(params),
    })
}

fn proxy_mode(params: &serde_json::Map<String, Value>) -> bool {
    params.get("mode").and_then(Value::as_str) == Some("proxy")
        || params["launch"]
            .get("invocation_mode")
            .and_then(Value::as_str)
            == Some("proxy")
}

fn append_system_prompt_override(launch: &Value, argv: &mut Vec<String>) {
    let Some(system_prompt) = system_prompt_override(launch) else {
        return;
    };
    argv.push("--append-system-prompt".to_string());
    argv.push(system_prompt.to_string());
}

fn append_prompt_arg(prompt: &prompt::PromptProjection, argv: &mut Vec<String>) {
    let Some(prompt) = &prompt.prompt else {
        return;
    };
    argv.push(prompt.clone());
}

fn system_prompt_override(launch: &Value) -> Option<&str> {
    system_prompt_override_value(launch).filter(|value| non_empty_string(value))
}

fn system_prompt_override_value(launch: &Value) -> Option<&str> {
    launch.get("system_prompt_override").and_then(Value::as_str)
}

fn non_empty_string(value: &str) -> bool {
    !value.is_empty()
}

fn policy_response(
    argv: Vec<String>,
    prompt: prompt::PromptProjection,
    diagnostics: Vec<Value>,
) -> Value {
    let accepted = policy_accepted(&diagnostics);
    format_policy_response(accepted, argv, prompt, diagnostics)
}

fn policy_accepted(diagnostics: &[Value]) -> bool {
    diagnostics.is_empty()
}

fn format_policy_response(
    accepted: bool,
    argv: Vec<String>,
    prompt: prompt::PromptProjection,
    diagnostics: Vec<Value>,
) -> Value {
    serde_json::json!({
        "accepted": accepted,
        "argv": argv,
        "env": {},
        "stdin": prompt.stdin,
        "prompt": prompt.prompt,
        "diagnostics": diagnostics,
        "markers": []
    })
}

fn provider_args(model: &Value) -> Option<Vec<String>> {
    let object = model.as_object()?;
    object.get("name")?.as_str()?;
    object.get("inputs")?.as_object()?;
    object
        .get("provider_args")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_string))
        .collect()
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_policy_evaluate_params",
        "policy.evaluate params do not match the policy contract",
    )
}
