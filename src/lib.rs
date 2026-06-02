use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, DirEntry, File};
use std::io::{BufRead, BufReader};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CONTRACT: &str = "oulipoly.provider/v1";
pub const SETTINGS_SCHEMA_ID: &str = "claude.settings/v1";
const SETTINGS_SCHEMA_URI: &str = "https://oulipoly.dev/schemas/claude.settings/v1";

const KNOWN_LATER_SUBCOMMANDS: &[&str] = &[
    "settings.list",
    "settings.get",
    "settings.create",
    "settings.update",
    "settings.delete",
    "settings.validate",
    "settings.migrate",
    "rotation.assess",
    "rotation.materialize",
    "discovery.models",
    "discovery.accounts",
    "setup.detect",
    "setup.install_plan",
    "setup.sync_plan",
    "setup_brain.turn",
    "migration.plan",
    "migration.apply",
];

#[derive(Debug, PartialEq, Eq)]
pub struct InvocationOutput {
    pub stdout: String,
    pub exit_code: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    contract: String,
    request_id: String,
    #[allow(dead_code)]
    provider_instance_id: Option<String>,
    host: HostContext,
    params: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostContext {
    app: String,
    #[allow(dead_code)]
    app_version: Option<String>,
    #[allow(dead_code)]
    platform: Option<String>,
    #[allow(dead_code)]
    working_directory: Option<String>,
    #[allow(dead_code)]
    config_root: Option<String>,
    #[allow(dead_code)]
    data_root: Option<String>,
    #[allow(dead_code)]
    env: Option<BTreeMap<String, String>>,
    #[allow(dead_code)]
    deadline_unix_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaParams {
    schema_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderModelRequest {
    #[allow(dead_code)]
    name: String,
    provider_args: Vec<String>,
    inputs: ModelInputs,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelInputs {
    prompt: Option<String>,
    #[allow(dead_code)]
    named: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyEvaluateParams {
    #[allow(dead_code)]
    settings_id: String,
    mode: String,
    model: ProviderModelRequest,
    launch: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchParams {
    #[allow(dead_code)]
    settings_id: String,
    #[allow(dead_code)]
    mode: String,
    #[allow(dead_code)]
    model: ProviderModelRequest,
    argv: Vec<String>,
    working_directory: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    stdin: Option<BytePayload>,
    #[allow(dead_code)]
    session: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalClassifyParams {
    stdout_base64: String,
    stderr_base64: String,
    status: ProcessStatus,
    observed_at_unix_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaBaseParams {
    settings_id: String,
    #[allow(dead_code)]
    model_name: Option<String>,
    context: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuotaRefreshAuthParams {
    settings_id: String,
    #[allow(dead_code)]
    force: Option<bool>,
    context: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionParams {
    settings_id: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    model_name: Option<String>,
    #[serde(default)]
    provider_name: Option<String>,
    #[serde(default)]
    context: Option<Value>,
    #[serde(default)]
    canonical_format: Option<String>,
    #[serde(default)]
    data_base64: Option<String>,
    #[serde(default)]
    preimage_sha256: Option<String>,
    #[serde(default)]
    stdout_base64: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    stderr_base64: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    state: Option<Value>,
    #[serde(default)]
    capture: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct QuotaScriptOutput {
    #[serde(default)]
    windows: Option<Vec<QuotaScriptWindow>>,
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaScriptWindow {
    #[serde(default)]
    window_id: u32,
    used_percent: f64,
    resets_at: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CachedQuotaWindow {
    #[allow(dead_code)]
    name: Option<String>,
    #[allow(dead_code)]
    remaining_ratio: f64,
    resets_at_unix_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BytePayload {
    encoding: String,
    data: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ProcessStatus {
    Exited { code: i32 },
    SignalTerminated { signal: i32 },
    SpawnError { reason: String },
    ProlongedSilence { reason: String },
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalSignalKind {
    CleanExit,
    NonzeroExit,
    SignalExit,
    SpawnError,
    QuotaExhaustedInband,
    #[allow(dead_code)]
    MaybeQuotaExhausted,
    RateLimited,
    ProlongedSilence,
    Cancelled,
    Unknown,
}

#[derive(Debug)]
struct TerminalSignal {
    kind: TerminalSignalKind,
    evidence: Option<String>,
    observed_at_unix_ms: u64,
}

#[derive(Debug)]
enum DrainEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

#[derive(Debug, Deserialize)]
struct LaunchPolicy {
    #[serde(default = "default_command")]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    interactive_args: Option<Vec<String>>,
    #[serde(default = "default_prompt_mode")]
    prompt_mode: String,
    #[serde(default = "default_invocation_mode")]
    invocation_mode: String,
    #[serde(default)]
    argv: Option<Vec<String>>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    system_prompt_override: Option<String>,
    #[serde(default)]
    tool_restrictions: Option<ToolRestrictions>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolRestrictions {
    #[serde(default)]
    claude: ClaudeRestrictions,
}

#[derive(Debug, Default, Deserialize)]
struct ClaudeRestrictions {
    #[serde(default)]
    disallowed_tools: Vec<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    disable_slash_commands: bool,
}

#[derive(Debug)]
struct ProviderFailure {
    request_id: String,
    code: &'static str,
    category: &'static str,
    message: String,
    retryable: bool,
    details: Value,
    exit_code: i32,
}

enum ProviderReply {
    Json(Value),
    Raw(String),
}

impl ProviderFailure {
    fn invalid_request(request_id: String, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            request_id,
            code,
            category: "invalid_request",
            message: message.into(),
            retryable: false,
            details: json!({}),
            exit_code: 2,
        }
    }

    fn unsupported(
        request_id: String,
        code: &'static str,
        message: impl Into<String>,
        exit_code: i32,
    ) -> Self {
        Self {
            request_id,
            code,
            category: "unsupported",
            message: message.into(),
            retryable: false,
            details: json!({}),
            exit_code,
        }
    }

    fn unavailable(
        request_id: String,
        code: &'static str,
        message: impl Into<String>,
        retryable: bool,
        details: Value,
    ) -> Self {
        Self {
            request_id,
            code,
            category: "unavailable",
            message: message.into(),
            retryable,
            details,
            exit_code: 1,
        }
    }
}

pub fn handle_invocation(args: &[String], stdin: &str) -> InvocationOutput {
    match handle_invocation_result(args, stdin) {
        Ok(ProviderReply::Json(value)) => InvocationOutput {
            stdout: serde_json::to_string(&value).expect("response serialization is infallible"),
            exit_code: 0,
        },
        Ok(ProviderReply::Raw(stdout)) => InvocationOutput {
            stdout,
            exit_code: 0,
        },
        Err(failure) => InvocationOutput {
            stdout: error_response(&failure),
            exit_code: failure.exit_code,
        },
    }
}

pub fn write_invocation<W: Write>(args: &[String], stdin: &str, writer: &mut W) -> i32 {
    match write_invocation_result(args, stdin, writer) {
        Ok(exit_code) => exit_code,
        Err(failure) => {
            let _ = writer.write_all(error_response(&failure).as_bytes());
            failure.exit_code
        }
    }
}

fn write_invocation_result<W: Write>(
    args: &[String],
    stdin: &str,
    writer: &mut W,
) -> Result<i32, ProviderFailure> {
    let request = decode_request(stdin)?;
    let subcommand = subcommand_from_args(args, request.request_id.clone())?;
    if subcommand == "launch" {
        let request_id = request.request_id.clone();
        let params = decode_launch_params(request)?;
        stream_launch(&request_id, params, writer)?;
        return Ok(0);
    }

    let response = handle_decoded_invocation(request, subcommand)?;
    writer
        .write_all(
            serde_json::to_string(&response)
                .expect("response serialization is infallible")
                .as_bytes(),
        )
        .map_err(|err| ProviderFailure {
            request_id: "unknown".to_string(),
            code: "stdout_write_failed",
            category: "failed",
            message: format!("failed to write provider response to stdout: {err}"),
            retryable: false,
            details: json!({}),
            exit_code: 1,
        })?;
    Ok(0)
}

fn handle_invocation_result(
    args: &[String],
    stdin: &str,
) -> Result<ProviderReply, ProviderFailure> {
    let request = decode_request(stdin)?;
    let subcommand = subcommand_from_args(args, request.request_id.clone())?;
    if subcommand == "launch" {
        let request_id = request.request_id.clone();
        let params = decode_launch_params(request)?;
        return Ok(ProviderReply::Raw(run_launch(&request_id, params)));
    }

    handle_decoded_invocation(request, subcommand).map(ProviderReply::Json)
}

fn handle_decoded_invocation(
    request: RequestEnvelope,
    subcommand: &str,
) -> Result<Value, ProviderFailure> {
    match subcommand {
        "describe" => Ok(success_response(&request.request_id, describe_result())),
        "schema" => schema_response(request),
        "policy.evaluate" => policy_evaluate_response(request),
        "terminal.classify" => terminal_classify_response(request),
        "quota.source" => quota_source_response(request),
        "quota.probe" => quota_probe_response(request),
        "quota.refresh_auth" => quota_refresh_auth_response(request),
        "session.locate_transcript" => session_locate_transcript_response(request),
        "session.read_turns" => session_read_turns_response(request),
        "session.capture" => session_capture_response(request),
        "session.export" => session_export_response(request),
        "session.replace" => session_replace_response(request),
        known if KNOWN_LATER_SUBCOMMANDS.contains(&known) => Err(ProviderFailure::unsupported(
            request.request_id,
            "capability_not_implemented",
            format!("{known} is advertised for the Claude provider but is not implemented in this foundation slice"),
            3,
        )),
        unknown => Err(ProviderFailure::unsupported(
            request.request_id,
            "unsupported_subcommand",
            format!("unsupported provider subcommand: {unknown}"),
            3,
        )),
    }
}

fn subcommand_from_args(args: &[String], request_id: String) -> Result<&str, ProviderFailure> {
    match args {
        [_, subcommand] => Ok(subcommand.as_str()),
        [_] => Err(ProviderFailure::unsupported(
            request_id,
            "missing_subcommand",
            "provider invocation requires exactly one subcommand argument",
            3,
        )),
        _ => Err(ProviderFailure::invalid_request(
            request_id,
            "invalid_argv",
            "provider invocation accepts exactly one subcommand argument",
        )),
    }
}

fn decode_request(stdin: &str) -> Result<RequestEnvelope, ProviderFailure> {
    let raw: Value = serde_json::from_str(stdin).map_err(|err| {
        ProviderFailure::invalid_request(
            "unknown".to_string(),
            "invalid_json",
            format!("stdin must be one UTF-8 JSON object: {err}"),
        )
    })?;
    let request_id = raw
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();
    let request: RequestEnvelope = serde_json::from_value(raw).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_envelope",
            format!("request envelope does not match the provider contract: {err}"),
        )
    })?;

    if request.contract != CONTRACT {
        return Err(ProviderFailure::unsupported(
            request.request_id,
            "unsupported_version",
            format!("unsupported contract version: {}", request.contract),
            3,
        ));
    }
    if request.request_id.trim().is_empty() {
        return Err(ProviderFailure::invalid_request(
            "unknown".to_string(),
            "invalid_request_id",
            "request_id must be a non-empty string",
        ));
    }
    if request.host.app.trim().is_empty() {
        return Err(ProviderFailure::invalid_request(
            request.request_id,
            "invalid_host",
            "host.app must be a non-empty string",
        ));
    }

    Ok(request)
}

fn schema_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params: SchemaParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request.request_id.clone(),
            "invalid_schema_params",
            format!("schema params must contain schema_id only: {err}"),
        )
    })?;
    if params.schema_id != SETTINGS_SCHEMA_ID {
        return Err(ProviderFailure::unsupported(
            request.request_id,
            "unknown_schema",
            format!("unknown provider schema id: {}", params.schema_id),
            1,
        ));
    }

    Ok(success_response(
        &request.request_id,
        json!({
            "schema_id": SETTINGS_SCHEMA_ID,
            "schema": settings_schema(),
            "ui": settings_schema_ui(),
        }),
    ))
}

fn policy_evaluate_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let params: PolicyEvaluateParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_policy_params",
            format!("policy.evaluate params do not match the provider contract: {err}"),
        )
    })?;
    let policy = launch_policy_from_value(&params.launch).map_err(|message| {
        ProviderFailure::invalid_request(request_id.clone(), "invalid_policy_launch", message)
    })?;

    let mut diagnostics = Vec::new();
    let mut argv = policy
        .argv
        .clone()
        .unwrap_or_else(|| policy_base_argv(&policy, &params));
    validate_policy(&policy, &argv, &mut diagnostics);
    append_claude_provider_policy(&policy, &mut argv);

    let accepted = !diagnostics.iter().any(|diagnostic| {
        diagnostic
            .get("severity")
            .and_then(Value::as_str)
            .is_some_and(|severity| severity == "error")
    });
    let stdin = policy.stdin.clone().or_else(|| {
        (policy.prompt_mode == "stdin")
            .then(|| params.model.inputs.prompt.clone())
            .flatten()
    });
    let prompt = (policy.prompt_mode == "arg")
        .then(|| params.model.inputs.prompt.clone())
        .flatten();

    Ok(success_response(
        &request_id,
        json!({
            "accepted": accepted,
            "argv": argv,
            "env": policy.env,
            "stdin": stdin,
            "prompt": prompt,
            "diagnostics": diagnostics,
            "markers": [],
        }),
    ))
}

fn terminal_classify_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let params: TerminalClassifyParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_terminal_params",
            format!("terminal.classify params do not match the provider contract: {err}"),
        )
    })?;
    let stdout = decode_base64(&params.stdout_base64).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_stdout_base64",
            format!("stdout_base64 is invalid: {err}"),
        )
    })?;
    let stderr = decode_base64(&params.stderr_base64).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_stderr_base64",
            format!("stderr_base64 is invalid: {err}"),
        )
    })?;
    let signal =
        classify_terminal_signal(&stdout, &stderr, &params.status, params.observed_at_unix_ms);

    Ok(success_response(
        &request_id,
        json!({ "terminal_signal": terminal_signal_json(&signal) }),
    ))
}

fn quota_source_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params: QuotaBaseParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_quota_source_params",
            format!("quota.source params do not match the provider contract: {err}"),
        )
    })?;
    validate_settings_id(&request_id, &params.settings_id)?;
    validate_quota_context(&request_id, &params.context)?;
    let source = quota_script_for_request(
        &params.context,
        &params.settings_id,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    );
    let has_source = source.is_some();
    let freshness = quota_source_freshness(has_source, &params.context);
    let mut result = json!({
        "has_source": has_source,
        "freshness": freshness,
    });
    if has_source {
        result["source_id"] = json!(quota_source_id(&params.settings_id));
    }

    Ok(success_response(&request_id, result))
}

fn quota_probe_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params: QuotaBaseParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_quota_probe_params",
            format!("quota.probe params do not match the provider contract: {err}"),
        )
    })?;
    validate_settings_id(&request_id, &params.settings_id)?;
    validate_quota_context(&request_id, &params.context)?;
    let script = quota_script_for_request(
        &params.context,
        &params.settings_id,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    )
    .ok_or_else(|| {
        ProviderFailure::unavailable(
            request_id.clone(),
            "quota_source_unavailable",
            "quota.probe requires quota_script in context or providers.toml",
            false,
            json!({ "settings_id": params.settings_id }),
        )
    })?;

    let checked_at = context_u64(&params.context, "now_unix_ms").unwrap_or_else(now_unix_ms);
    let stdout = run_shell_command(&script, Duration::from_secs(30), CommandKind::Quota)
        .map_err(|failure| command_failure(&request_id, failure, "quota_probe_failed"))?;
    let windows = parse_quota_script_output(&stdout).map_err(|message| {
        ProviderFailure::unavailable(
            request_id.clone(),
            "quota_probe_parse_failed",
            message,
            true,
            json!({}),
        )
    })?;
    if windows.is_empty() && context_has_prior_windows(&params.context) {
        return Err(ProviderFailure::unavailable(
            request_id,
            "quota_probe_empty_after_prior_data",
            "quota script returned empty windows after prior populated quota data",
            true,
            json!({ "refresh_auth_recommended": true }),
        ));
    }

    Ok(success_response(
        &request_id,
        json!({
            "available": true,
            "checked_at_unix_ms": checked_at,
            "windows": windows,
            "detail": quota_probe_detail(&windows),
        }),
    ))
}

fn quota_refresh_auth_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params: QuotaRefreshAuthParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.clone(),
            "invalid_quota_refresh_auth_params",
            format!("quota.refresh_auth params do not match the provider contract: {err}"),
        )
    })?;
    validate_settings_id(&request_id, &params.settings_id)?;
    validate_quota_context(&request_id, &params.context)?;
    let Some(command) = auth_refresh_command_for_request(
        &params.context,
        &params.settings_id,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    ) else {
        return Err(ProviderFailure::unavailable(
            request_id,
            "quota_refresh_auth_unavailable",
            "quota.refresh_auth requires auth_refresh_command in context or providers.toml",
            false,
            json!({ "settings_id": params.settings_id }),
        ));
    };

    run_shell_command(&command, Duration::from_secs(15), CommandKind::Auth)
        .map_err(|failure| command_failure(&request_id, failure, "quota_refresh_auth_failed"))?;
    Ok(success_response(
        &request_id,
        json!({
            "refreshed": true,
            "available": true,
            "checked_at_unix_ms": context_u64(&params.context, "now_unix_ms").unwrap_or_else(now_unix_ms),
            "detail": "token refreshed",
        }),
    ))
}

fn session_locate_transcript_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params = decode_session_params(request, "invalid_session_locate_params")?;
    validate_settings_id(&request_id, &params.settings_id)?;
    let session_id = require_session_id(&request_id, &params)?;
    let settings = session_settings_for_request(
        &params,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    );
    let located = locate_transcript(&request_id, &settings, &session_id)?;

    Ok(success_response(
        &request_id,
        json!({
            "located": true,
            "path": located.display().to_string(),
            "format_id": "claude_code",
            "source_id": format!("claude:{}", params.settings_id),
            "require_existing_observed": true,
        }),
    ))
}

fn session_read_turns_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params = decode_session_params(request, "invalid_session_read_turns_params")?;
    validate_settings_id(&request_id, &params.settings_id)?;
    let session_id = require_session_id(&request_id, &params)?;
    let settings = session_settings_for_request(
        &params,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    );
    let located = locate_transcript(&request_id, &settings, &session_id)?;
    let mut turns = read_claude_turns(&request_id, &located, &session_id)?;
    if let Some(after) = session_context_string(&params, "after_turn_id") {
        if let Some(index) = turns
            .iter()
            .position(|turn| turn.get("turn_id").and_then(Value::as_str) == Some(after.as_str()))
        {
            turns = turns.split_off(index + 1);
        }
    }

    Ok(success_response(
        &request_id,
        json!({
            "turn_count": turns.len(),
            "turns": turns,
            "complete": transcript_is_complete(&located),
        }),
    ))
}

fn session_capture_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params = decode_session_params(request, "invalid_session_capture_params")?;
    validate_settings_id(&request_id, &params.settings_id)?;
    let settings = session_settings_for_request(
        &params,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    );
    let capture = params
        .capture
        .as_ref()
        .and_then(|value| serde_json::from_value::<SessionCaptureSettings>(value.clone()).ok())
        .or(settings.capture)
        .unwrap_or_default();
    let result = capture_result(&request_id, &params, &capture)?;
    Ok(success_response(&request_id, result))
}

fn session_export_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params = decode_session_params(request, "invalid_session_export_params")?;
    validate_settings_id(&request_id, &params.settings_id)?;
    let session_id = require_session_id(&request_id, &params)?;
    let settings = session_settings_for_request(
        &params,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    );
    let located = locate_transcript(&request_id, &settings, &session_id)?;
    let provider_name = provider_name_for_session(&params);
    let records =
        canonical_records_from_claude_file(&request_id, &located, &session_id, &provider_name)?;
    let bytes = canonical_jsonl_bytes(&request_id, &records)?;

    Ok(success_response(
        &request_id,
        json!({
            "canonical_format": "oulipoly.canonical_transcript/v1",
            "data_base64": encode_base64(&bytes),
            "turn_count": records.len(),
            "sha256": sha256_hex(&bytes),
        }),
    ))
}

fn session_replace_response(request: RequestEnvelope) -> Result<Value, ProviderFailure> {
    let request_id = request.request_id.clone();
    let config_root = request.host.config_root.clone();
    let provider_instance_id = request.provider_instance_id.clone();
    let params = decode_session_params(request, "invalid_session_replace_params")?;
    validate_settings_id(&request_id, &params.settings_id)?;
    let session_id = require_session_id(&request_id, &params)?;
    validate_canonical_format(&request_id, &params)?;
    let replacement_bytes = replacement_bytes(&request_id, &params)?;
    let replacement_records = parse_canonical_jsonl(&request_id, &replacement_bytes)?;
    validate_replacement_records(
        &request_id,
        &replacement_records,
        &session_id,
        &provider_name_for_session(&params),
    )?;
    let settings = session_settings_for_request(
        &params,
        provider_instance_id.as_deref(),
        config_root.as_deref(),
    );
    let located = locate_transcript(&request_id, &settings, &session_id)?;
    let provider_name = provider_name_for_session(&params);
    let existing_records =
        canonical_records_from_claude_file(&request_id, &located, &session_id, &provider_name)?;
    let existing_bytes = canonical_jsonl_bytes(&request_id, &existing_records)?;
    let existing_hash = sha256_hex(&existing_bytes);
    if let Some(expected) = params.preimage_sha256.as_deref() {
        if expected != existing_hash {
            return Err(ProviderFailure::unavailable(
                request_id,
                "preimage_mismatch",
                "session.replace preimage_sha256 does not match current canonical transcript",
                false,
                json!({ "expected": expected, "actual": existing_hash }),
            ));
        }
    }
    let replacement_hash = sha256_hex(&replacement_bytes);
    if replacement_bytes == existing_bytes {
        return Ok(success_response(
            &request_id,
            json!({
                "changed": false,
                "artifacts": [session_artifact(&located, &replacement_hash)],
            }),
        ));
    }

    let rendered = render_claude_records(&request_id, &replacement_records)?;
    atomic_replace_file(&request_id, &located, &rendered)?;
    let fresh_records =
        canonical_records_from_claude_file(&request_id, &located, &session_id, &provider_name)?;
    let fresh_bytes = canonical_jsonl_bytes(&request_id, &fresh_records)?;
    let postimage_hash = sha256_hex(&fresh_bytes);
    if !canonical_semantics_equal(&replacement_records, &fresh_records) {
        return Err(ProviderFailure::unavailable(
            request_id,
            "postimage_mismatch",
            "fresh canonical export after replace does not match replacement semantics",
            false,
            json!({ "replacement_sha256": replacement_hash, "postimage_sha256": postimage_hash }),
        ));
    }
    let artifacts = vec![session_artifact(&located, &postimage_hash)];

    Ok(success_response(
        &request_id,
        json!({
            "changed": true,
            "postimage_sha256": postimage_hash,
            "artifacts": artifacts,
            "host_state_plan": {
                "schema_version": 1,
                "operation": "session.replace",
                "session_id": session_id,
                "provider_name": provider_name,
                "canonical_format": "oulipoly.canonical_transcript/v1",
                "turn_count": fresh_records.len(),
                "records_sha256": replacement_hash,
                "postimage_sha256": postimage_hash,
                "artifacts": artifacts,
            },
        }),
    ))
}

fn decode_launch_params(request: RequestEnvelope) -> Result<LaunchParams, ProviderFailure> {
    let params: LaunchParams = serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request.request_id.clone(),
            "invalid_launch_params",
            format!("launch params do not match the provider contract: {err}"),
        )
    })?;
    Ok(params)
}

fn run_launch(request_id: &str, params: LaunchParams) -> String {
    let mut output = Vec::new();
    let _ = stream_launch(request_id, params, &mut output);
    String::from_utf8(output).expect("launch JSONL stream is UTF-8")
}

fn stream_launch<W: Write>(
    request_id: &str,
    params: LaunchParams,
    writer: &mut W,
) -> Result<(), ProviderFailure> {
    let mut stream = LaunchStream::new(request_id, writer);
    if params.argv.is_empty() {
        let status = ProcessStatus::SpawnError {
            reason: "Empty command".to_string(),
        };
        let signal = classify_terminal_signal(&[], &[], &status, now_unix_ms());
        stream.exit(status, signal, None);
        return Ok(());
    }
    let stdin_payload = match params.stdin.as_ref().map(byte_payload_bytes).transpose() {
        Ok(payload) => payload,
        Err(reason) => {
            let status = ProcessStatus::SpawnError { reason };
            let signal = classify_terminal_signal(&[], &[], &status, now_unix_ms());
            stream.exit(status, signal, None);
            return Ok(());
        }
    };
    let session = params.session.clone();
    if session
        .as_ref()
        .and_then(|value| value.get("provider_session_id"))
        .and_then(Value::as_str)
        .is_some()
    {
        stream.marker("provider_session_known");
    }

    let mut command = Command::new(&params.argv[0]);
    command.args(&params.argv[1..]);
    command.current_dir(&params.working_directory);
    command.envs(params.env);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command.stdin(if stdin_payload.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });

    match command.spawn() {
        Ok(mut child) => {
            if let Some(payload) = stdin_payload {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(&payload);
                    let _ = stdin.flush();
                }
            }

            let (tx, rx) = mpsc::channel();
            let mut drains = Vec::new();
            if let Some(stdout) = child.stdout.take() {
                drains.push(spawn_drain(stdout, tx.clone(), DrainKind::Stdout));
            }
            if let Some(stderr) = child.stderr.take() {
                drains.push(spawn_drain(stderr, tx.clone(), DrainKind::Stderr));
            }
            drop(tx);

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let status = loop {
                drain_available_events(&rx, &mut stream, &mut stdout, &mut stderr);
                match child.try_wait() {
                    Ok(Some(status)) => break process_status_from_output(&status),
                    Ok(None) => match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(event) => drain_one_event(event, &mut stream, &mut stdout, &mut stderr),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => {}
                    },
                    Err(err) => {
                        break ProcessStatus::SpawnError {
                            reason: format!("Failed to supervise Claude provider child: {err}"),
                        };
                    }
                }
            };
            for drain in drains {
                let _ = drain.join();
            }
            drain_available_events(&rx, &mut stream, &mut stdout, &mut stderr);
            let signal = classify_terminal_signal(&stdout, &stderr, &status, now_unix_ms());
            stream.exit(status, signal, session);
        }
        Err(err) => {
            let status = ProcessStatus::SpawnError {
                reason: format!("Failed to spawn Claude provider command: {err}"),
            };
            let signal = classify_terminal_signal(&[], &[], &status, now_unix_ms());
            stream.exit(status, signal, session);
        }
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum DrainKind {
    Stdout,
    Stderr,
}

fn spawn_drain<R: Read + Send + 'static>(
    mut reader: R,
    tx: mpsc::Sender<DrainEvent>,
    kind: DrainKind,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let chunk = buffer[..count].to_vec();
                    let event = match kind {
                        DrainKind::Stdout => DrainEvent::Stdout(chunk),
                        DrainKind::Stderr => DrainEvent::Stderr(chunk),
                    };
                    if tx.send(event).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn drain_available_events<W: Write>(
    rx: &mpsc::Receiver<DrainEvent>,
    stream: &mut LaunchStream<'_, W>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    while let Ok(event) = rx.try_recv() {
        drain_one_event(event, stream, stdout, stderr);
    }
}

fn drain_one_event<W: Write>(
    event: DrainEvent,
    stream: &mut LaunchStream<'_, W>,
    stdout: &mut Vec<u8>,
    stderr: &mut Vec<u8>,
) {
    match event {
        DrainEvent::Stdout(bytes) => {
            stream.bytes("stdout", &bytes);
            stdout.extend(bytes);
        }
        DrainEvent::Stderr(bytes) => {
            stream.bytes("stderr", &bytes);
            stderr.extend(bytes);
        }
    }
}

fn success_response(request_id: &str, result: Value) -> Value {
    json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "ok": true,
        "result": result,
    })
}

fn default_command() -> String {
    "env -u CLAUDECODE claude".to_string()
}

fn default_prompt_mode() -> String {
    "stdin".to_string()
}

fn default_invocation_mode() -> String {
    "headless".to_string()
}

fn launch_policy_from_value(value: &Value) -> Result<LaunchPolicy, String> {
    serde_json::from_value(value.clone())
        .map_err(|err| format!("launch policy settings are malformed: {err}"))
}

fn policy_base_argv(policy: &LaunchPolicy, params: &PolicyEvaluateParams) -> Vec<String> {
    let mut argv = shell_split(&policy.command);
    argv.extend(policy.args.iter().cloned());
    if params.mode == "proxy" {
        if let Some(interactive_args) = &policy.interactive_args {
            argv.extend(interactive_args.iter().cloned());
        }
    }
    argv.extend(params.model.provider_args.iter().cloned());
    if policy.prompt_mode == "arg" {
        if let Some(prompt) = &params.model.inputs.prompt {
            argv.push(prompt.clone());
        }
    }
    argv
}

fn append_claude_provider_policy(policy: &LaunchPolicy, argv: &mut Vec<String>) {
    if let Some(override_text) = &policy.system_prompt_override {
        argv.push("--append-system-prompt".to_string());
        argv.push(override_text.clone());
    }
    let Some(restrictions) = &policy.tool_restrictions else {
        return;
    };
    if !restrictions.claude.disallowed_tools.is_empty() {
        argv.push("--disallowed-tools".to_string());
        argv.push(restrictions.claude.disallowed_tools.join(","));
    }
    if !restrictions.claude.allowed_tools.is_empty() {
        argv.push("--allowed-tools".to_string());
        argv.push(restrictions.claude.allowed_tools.join(","));
    }
    if restrictions.claude.disable_slash_commands {
        argv.push("--disable-slash-commands".to_string());
    }
}

fn validate_policy(policy: &LaunchPolicy, argv: &[String], diagnostics: &mut Vec<Value>) {
    let Some(restrictions) = &policy.tool_restrictions else {
        validate_proxy_filter_shape(policy, argv, diagnostics);
        return;
    };
    if !restrictions.claude.allowed_tools.is_empty()
        && !restrictions.claude.disallowed_tools.is_empty()
    {
        diagnostics.push(diagnostic(
            "error",
            "tool_restrictions.claude.allowed_tools and disallowed_tools are mutually exclusive",
            "claude_tool_restrictions_mutually_exclusive",
        ));
    }
    validate_duplicate_claude_tools(
        argv,
        "tool_restrictions.claude.allowed_tools",
        &["--allowedTools", "--allowed-tools"],
        &restrictions.claude.allowed_tools,
        diagnostics,
    );
    validate_duplicate_claude_tools(
        argv,
        "tool_restrictions.claude.disallowed_tools",
        &["--disallowedTools", "--disallowed-tools"],
        &restrictions.claude.disallowed_tools,
        diagnostics,
    );
    validate_proxy_filter_shape(policy, argv, diagnostics);
}

fn validate_duplicate_claude_tools(
    argv: &[String],
    policy_field: &str,
    flags: &[&str],
    policy_tools: &[String],
    diagnostics: &mut Vec<Value>,
) {
    for (flag, raw_value) in flag_values(argv, flags) {
        for raw_tool in raw_value
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
        {
            if policy_tools.iter().any(|tool| tool == raw_tool) {
                diagnostics.push(diagnostic(
                    "error",
                    &format!(
                        "{policy_field} contains duplicate tool {raw_tool:?} already present in argv flag {flag}"
                    ),
                    "duplicate_claude_tool_filter",
                ));
            }
        }
    }
}

fn validate_proxy_filter_shape(
    policy: &LaunchPolicy,
    argv: &[String],
    diagnostics: &mut Vec<Value>,
) {
    if policy.invocation_mode != "proxy" {
        return;
    }
    if flag_values(argv, &["--tools"])
        .into_iter()
        .any(|(_, value)| value.starts_with("mcp__"))
    {
        diagnostics.push(diagnostic(
            "error",
            "proxy-mode Claude must not use `--tools mcp__...`; use `--allowedTools` or omit the filter",
            "unsafe_proxy_claude_tools_restrict",
        ));
    }
}

fn validate_settings_id(request_id: &str, settings_id: &str) -> Result<(), ProviderFailure> {
    if !settings_id.trim().is_empty() {
        return Ok(());
    }
    Err(ProviderFailure::invalid_request(
        request_id.to_string(),
        "invalid_settings_id",
        "settings_id must be non-empty",
    ))
}

fn validate_quota_context(
    request_id: &str,
    context: &Option<Value>,
) -> Result<(), ProviderFailure> {
    if context.as_ref().is_none_or(Value::is_object) {
        return Ok(());
    }
    Err(ProviderFailure::invalid_request(
        request_id.to_string(),
        "invalid_quota_context",
        "quota context must be a JSON object when supplied",
    ))
}

fn quota_source_id(settings_id: &str) -> String {
    format!("claude:{settings_id}:quota_script")
}

fn quota_script_from_context(context: &Option<Value>) -> Option<String> {
    context_string(context, "quota_script").filter(|value| !value.trim().is_empty())
}

fn auth_refresh_command_from_context(context: &Option<Value>) -> Option<String> {
    context_string(context, "auth_refresh_command").filter(|value| !value.trim().is_empty())
}

fn quota_script_for_request(
    context: &Option<Value>,
    settings_id: &str,
    provider_instance_id: Option<&str>,
    config_root: Option<&str>,
) -> Option<String> {
    quota_script_from_context(context).or_else(|| {
        provider_quota_settings(context, settings_id, provider_instance_id, config_root)
            .and_then(|settings| settings.quota_script)
    })
}

fn auth_refresh_command_for_request(
    context: &Option<Value>,
    settings_id: &str,
    provider_instance_id: Option<&str>,
    config_root: Option<&str>,
) -> Option<String> {
    auth_refresh_command_from_context(context).or_else(|| {
        provider_quota_settings(context, settings_id, provider_instance_id, config_root)
            .and_then(|settings| settings.auth_refresh_command)
    })
}

#[derive(Default)]
struct ProviderQuotaSettings {
    quota_script: Option<String>,
    auth_refresh_command: Option<String>,
}

#[derive(Default)]
struct ProviderSessionSettings {
    storage: Option<SessionStorage>,
    capture: Option<SessionCaptureSettings>,
}

#[derive(Clone)]
enum SessionStorage {
    ClaudeCode {
        projects_dir: PathBuf,
    },
    Script {
        locate_script: Option<String>,
        #[allow(dead_code)]
        turns_script: Option<String>,
    },
}

#[derive(Clone, Default, Deserialize)]
struct SessionCaptureSettings {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    flag: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    json_flag: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    last_message_flag: Option<String>,
    #[serde(default)]
    event_type: Option<String>,
    #[serde(default)]
    event_id_path: Option<String>,
    #[serde(default)]
    provider_session_id: Option<String>,
    #[serde(default)]
    start_known_provider_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CanonicalRecord {
    session_id: String,
    provider_name: String,
    turn_id: String,
    role: String,
    timestamp: String,
    content: Vec<ContentChunk>,
    source: RecordSource,
    unsupported_record: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContentChunk {
    #[serde(rename = "type")]
    chunk_type: String,
    text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordSource {
    storage_type: String,
    jsonl_path: PathBuf,
    line: u64,
    byte_start: u64,
    byte_end: u64,
    sha256: String,
}

struct SourceLine {
    line: u64,
    byte_start: u64,
    byte_end: u64,
    sha256: String,
    value: Value,
}

fn provider_quota_settings(
    context: &Option<Value>,
    settings_id: &str,
    provider_instance_id: Option<&str>,
    config_root: Option<&str>,
) -> Option<ProviderQuotaSettings> {
    let config_root = config_root?;
    let providers_toml = std::path::Path::new(config_root).join("providers.toml");
    let candidates = provider_config_candidates(context, provider_instance_id, settings_id);
    let mut settings = std::fs::read_to_string(providers_toml)
        .ok()
        .and_then(|text| parse_provider_quota_settings(&text, &candidates))
        .unwrap_or_default();
    if settings.quota_script.is_none() {
        if let Some(legacy) = legacy_session_quota_settings(config_root, &candidates) {
            settings.quota_script = legacy.quota_script;
        }
    }
    active_has_quota(&settings).then_some(settings)
}

fn decode_session_params(
    request: RequestEnvelope,
    code: &'static str,
) -> Result<SessionParams, ProviderFailure> {
    serde_json::from_value(request.params).map_err(|err| {
        ProviderFailure::invalid_request(
            request.request_id,
            code,
            format!("session params do not match the provider contract: {err}"),
        )
    })
}

fn require_session_id(request_id: &str, params: &SessionParams) -> Result<String, ProviderFailure> {
    params
        .session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            ProviderFailure::invalid_request(
                request_id.to_string(),
                "invalid_session_id",
                "session_id must be non-empty",
            )
        })
}

fn validate_canonical_format(
    request_id: &str,
    params: &SessionParams,
) -> Result<(), ProviderFailure> {
    if params.canonical_format.as_deref() == Some("oulipoly.canonical_transcript/v1") {
        return Ok(());
    }
    Err(ProviderFailure::invalid_request(
        request_id.to_string(),
        "invalid_canonical_format",
        "session.replace requires canonical_format=oulipoly.canonical_transcript/v1",
    ))
}

fn replacement_bytes(request_id: &str, params: &SessionParams) -> Result<Vec<u8>, ProviderFailure> {
    let data = params.data_base64.as_deref().ok_or_else(|| {
        ProviderFailure::invalid_request(
            request_id.to_string(),
            "missing_replacement_data",
            "session.replace requires data_base64",
        )
    })?;
    decode_base64(data).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_replacement_base64",
            format!("data_base64 is invalid: {err}"),
        )
    })
}

fn provider_name_for_session(params: &SessionParams) -> String {
    params
        .provider_name
        .clone()
        .or_else(|| session_context_string(params, "provider_name"))
        .unwrap_or_else(|| params.settings_id.clone())
}

fn session_settings_for_request(
    params: &SessionParams,
    provider_instance_id: Option<&str>,
    config_root: Option<&str>,
) -> ProviderSessionSettings {
    let mut settings = ProviderSessionSettings::default();
    if let Some(storage) = session_storage_from_context(&params.context) {
        settings.storage = Some(storage);
    }
    if let Some(capture) = session_capture_from_context(&params.context) {
        settings.capture = Some(capture);
    }
    if settings.storage.is_some() && settings.capture.is_some() {
        return settings;
    }
    if let Some(from_config) = provider_session_settings(
        &params.context,
        &params.settings_id,
        provider_instance_id,
        config_root,
    ) {
        if settings.storage.is_none() {
            settings.storage = from_config.storage;
        }
        if settings.capture.is_none() {
            settings.capture = from_config.capture;
        }
    }
    settings
}

fn provider_session_settings(
    context: &Option<Value>,
    settings_id: &str,
    provider_instance_id: Option<&str>,
    config_root: Option<&str>,
) -> Option<ProviderSessionSettings> {
    let providers_toml = Path::new(config_root?).join("providers.toml");
    let parsed: toml::Value = toml::from_str(&fs::read_to_string(providers_toml).ok()?).ok()?;
    let candidates = provider_config_candidates(context, provider_instance_id, settings_id);
    candidates
        .iter()
        .map(|candidate| parse_provider_session_settings_for_candidate(&parsed, candidate))
        .find(|settings| settings.storage.is_some() || settings.capture.is_some())
}

fn parse_provider_session_settings_for_candidate(
    providers_toml: &toml::Value,
    candidate: &str,
) -> ProviderSessionSettings {
    let mut settings = ProviderSessionSettings::default();
    let Some(table) = providers_toml.get(candidate) else {
        return settings;
    };
    settings.storage = table
        .get("session_storage")
        .and_then(session_storage_from_toml);
    settings.capture = table
        .get("session_capture")
        .and_then(|value| value.clone().try_into::<SessionCaptureSettings>().ok());
    settings
}

fn session_storage_from_context(context: &Option<Value>) -> Option<SessionStorage> {
    let storage = context
        .as_ref()
        .and_then(|value| nested_context_value(value, "session_storage"))?;
    session_storage_from_value(storage)
}

fn session_capture_from_context(context: &Option<Value>) -> Option<SessionCaptureSettings> {
    let capture = context
        .as_ref()
        .and_then(|value| nested_context_value(value, "session_capture"))?;
    serde_json::from_value(capture.clone()).ok()
}

fn session_storage_from_value(value: &Value) -> Option<SessionStorage> {
    let kind = value.get("kind").and_then(Value::as_str)?;
    match kind {
        "claude_code" => value
            .get("projects_dir")
            .and_then(Value::as_str)
            .map(|projects_dir| SessionStorage::ClaudeCode {
                projects_dir: expand_home_path(projects_dir),
            }),
        "script" => Some(SessionStorage::Script {
            locate_script: value
                .get("locate_script")
                .or_else(|| value.get("transcript_script"))
                .and_then(Value::as_str)
                .map(str::to_string),
            turns_script: value
                .get("turns_script")
                .or_else(|| value.get("turn_script"))
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        _ => None,
    }
}

fn session_storage_from_toml(value: &toml::Value) -> Option<SessionStorage> {
    let kind = table_string(value, "kind")?;
    match kind.as_str() {
        "claude_code" => {
            table_string(value, "projects_dir").map(|projects_dir| SessionStorage::ClaudeCode {
                projects_dir: expand_home_path(&projects_dir),
            })
        }
        "script" => Some(SessionStorage::Script {
            locate_script: table_string(value, "locate_script")
                .or_else(|| table_string(value, "transcript_script")),
            turns_script: table_string(value, "turns_script")
                .or_else(|| table_string(value, "turn_script")),
        }),
        _ => None,
    }
}

fn expand_home_path(path: &str) -> PathBuf {
    if path == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn locate_transcript(
    request_id: &str,
    settings: &ProviderSessionSettings,
    session_id: &str,
) -> Result<PathBuf, ProviderFailure> {
    match settings.storage.as_ref() {
        Some(SessionStorage::ClaudeCode { projects_dir }) => {
            locate_claude_project_transcript(request_id, projects_dir, session_id)
        }
        Some(SessionStorage::Script { locate_script, .. }) => {
            locate_transcript_with_script(request_id, locate_script.as_deref(), session_id)
        }
        None => Err(ProviderFailure::unavailable(
            request_id.to_string(),
            "session_storage_unavailable",
            "session operation requires session_storage in context or providers.toml",
            false,
            json!({}),
        )),
    }
}

fn locate_transcript_with_script(
    request_id: &str,
    locate_script: Option<&str>,
    session_id: &str,
) -> Result<PathBuf, ProviderFailure> {
    let Some(script) = locate_script else {
        return Err(ProviderFailure::unavailable(
            request_id.to_string(),
            "session_locator_unavailable",
            "script session_storage requires locate_script",
            false,
            json!({}),
        ));
    };
    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .env("SESSION_ID", session_id)
        .output()
        .map_err(|err| {
            ProviderFailure::unavailable(
                request_id.to_string(),
                "session_locator_failed",
                format!("failed to spawn transcript locator: {err}"),
                true,
                json!({}),
            )
        })?;
    if !output.status.success() {
        return Err(ProviderFailure::unavailable(
            request_id.to_string(),
            "session_not_found",
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            false,
            json!({ "session_id": session_id }),
        ));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    canonical_existing_jsonl(request_id, PathBuf::from(path))
}

fn locate_claude_project_transcript(
    request_id: &str,
    projects_dir: &Path,
    session_id: &str,
) -> Result<PathBuf, ProviderFailure> {
    let root = projects_dir.canonicalize().map_err(|err| {
        ProviderFailure::unavailable(
            request_id.to_string(),
            "claude_projects_dir_unavailable",
            format!(
                "failed to access Claude projects dir {}: {err}",
                projects_dir.display()
            ),
            false,
            json!({ "path": projects_dir.display().to_string() }),
        )
    })?;
    let target = format!("{session_id}.jsonl");
    let mut matches = Vec::new();
    collect_named_jsonl_matches(request_id, &root, &target, 0, &mut matches)?;
    if matches.is_empty() {
        collect_content_jsonl_matches(request_id, &root, session_id, 0, &mut matches)?;
    }
    match matches.len() {
        1 => canonical_existing_jsonl(request_id, matches.remove(0)),
        0 => Err(ProviderFailure::unavailable(
            request_id.to_string(),
            "session_not_found",
            format!("session not found: {session_id}"),
            false,
            json!({ "session_id": session_id }),
        )),
        _ => Err(ProviderFailure::unavailable(
            request_id.to_string(),
            "ambiguous_session",
            format!("multiple Claude transcripts matched session {session_id}"),
            false,
            json!({ "session_id": session_id }),
        )),
    }
}

fn collect_named_jsonl_matches(
    request_id: &str,
    dir: &Path,
    target: &str,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), ProviderFailure> {
    if depth > 4 {
        return Ok(());
    }
    for entry in read_dir_entries(request_id, dir)? {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_named_jsonl_matches(request_id, &path, target, depth + 1, matches)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some(target) && path.is_file()
        {
            matches.push(path);
        }
    }
    Ok(())
}

fn collect_content_jsonl_matches(
    request_id: &str,
    dir: &Path,
    session_id: &str,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), ProviderFailure> {
    if depth > 4 {
        return Ok(());
    }
    for entry in read_dir_entries(request_id, dir)? {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_content_jsonl_matches(request_id, &path, session_id, depth + 1, matches)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
            && file_contains_session_id(&path, session_id)
        {
            matches.push(path);
        }
    }
    Ok(())
}

fn read_dir_entries(request_id: &str, dir: &Path) -> Result<Vec<DirEntry>, ProviderFailure> {
    fs::read_dir(dir)
        .map_err(|err| {
            ProviderFailure::unavailable(
                request_id.to_string(),
                "session_storage_read_failed",
                format!(
                    "failed to read session storage dir {}: {err}",
                    dir.display()
                ),
                false,
                json!({ "path": dir.display().to_string() }),
            )
        })?
        .map(|entry| {
            entry.map_err(|err| {
                ProviderFailure::unavailable(
                    request_id.to_string(),
                    "session_storage_read_failed",
                    format!(
                        "failed to read session storage entry in {}: {err}",
                        dir.display()
                    ),
                    false,
                    json!({ "path": dir.display().to_string() }),
                )
            })
        })
        .collect()
}

fn canonical_existing_jsonl(request_id: &str, path: PathBuf) -> Result<PathBuf, ProviderFailure> {
    let path = path.canonicalize().map_err(|err| {
        ProviderFailure::unavailable(
            request_id.to_string(),
            "transcript_unavailable",
            format!(
                "failed to canonicalize transcript {}: {err}",
                path.display()
            ),
            false,
            json!({ "path": path.display().to_string() }),
        )
    })?;
    if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") && path.is_file() {
        return Ok(path);
    }
    Err(ProviderFailure::unavailable(
        request_id.to_string(),
        "invalid_transcript_path",
        "located transcript must be an existing .jsonl file",
        false,
        json!({ "path": path.display().to_string() }),
    ))
}

fn file_contains_session_id(path: &Path, session_id: &str) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .any(|line| {
            serde_json::from_str::<Value>(line.trim())
                .ok()
                .is_some_and(|value| {
                    value
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .is_some_and(|recorded| recorded == session_id)
                })
        })
}

fn read_claude_turns(
    request_id: &str,
    path: &Path,
    session_id: &str,
) -> Result<Vec<Value>, ProviderFailure> {
    let lines = scan_jsonl_file(request_id, path)?;
    let mut turns = Vec::new();
    for line in lines {
        if line.value.get("sessionId").and_then(Value::as_str) != Some(session_id) {
            continue;
        }
        let kind = line.value.get("type").and_then(Value::as_str);
        let compact = line
            .value
            .get("isCompactSummary")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !matches!(kind, Some("user" | "assistant")) && !compact {
            continue;
        }
        let Some(turn_id) = line.value.get("uuid").and_then(Value::as_str) else {
            continue;
        };
        let Some(timestamp) = line.value.get("timestamp").and_then(Value::as_str) else {
            continue;
        };
        let mut turn = json!({
            "session_id": session_id,
            "turn_id": turn_id,
            "timestamp": timestamp,
            "role": kind,
            "parent_turn_id": line.value.get("parentUuid").cloned().unwrap_or(Value::Null),
            "is_sidechain": line.value.get("isSidechain").cloned().unwrap_or(Value::Null),
            "is_compaction_boundary": compact,
            "status": if transcript_is_complete(path) { "complete" } else { "partial" },
        });
        let body = extract_claude_body_json(&line.value);
        if !body.is_empty() {
            turn["body"] = Value::Array(body);
        }
        turns.push(turn);
    }
    Ok(turns)
}

fn transcript_is_complete(path: &Path) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    bytes.is_empty() || bytes.last() == Some(&b'\n')
}

fn capture_result(
    request_id: &str,
    params: &SessionParams,
    capture: &SessionCaptureSettings,
) -> Result<Value, ProviderFailure> {
    if let Some(known) = capture
        .start_known_provider_session_id
        .as_deref()
        .or(capture.provider_session_id.as_deref())
        .or(params.session_id.as_deref())
        .filter(|value| !value.trim().is_empty() && capture.kind == "start_known")
    {
        return Ok(json!({
            "provider_session_id": known,
            "state": { "capture_kind": "start_known" },
            "artifacts": [],
        }));
    }
    match capture.kind.as_str() {
        "" | "none" => Ok(json!({
            "provider_session_id": Value::Null,
            "state": { "capture_kind": "none" },
            "artifacts": [],
        })),
        "forced_flag_verified" => {
            let requested = params
                .session_id
                .as_deref()
                .or(capture.provider_session_id.as_deref())
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ProviderFailure::invalid_request(
                        request_id.to_string(),
                        "missing_forced_capture_session_id",
                        "forced_flag_verified capture requires session_id or provider_session_id",
                    )
                })?;
            Ok(json!({
                "provider_session_id": requested,
                "state": {
                    "capture_kind": "forced_flag_verified",
                    "flag": capture.flag,
                    "verified": true,
                },
                "artifacts": [],
            }))
        }
        "stdout_json_event" => {
            let stdout = params
                .stdout_base64
                .as_deref()
                .map(decode_base64)
                .transpose()
                .map_err(|err| {
                    ProviderFailure::invalid_request(
                        request_id.to_string(),
                        "invalid_capture_stdout_base64",
                        format!("stdout_base64 is invalid: {err}"),
                    )
                })?
                .unwrap_or_default();
            let event_type = capture.event_type.as_deref().unwrap_or("system");
            let event_id_path = capture.event_id_path.as_deref().unwrap_or("session_id");
            let provider_session_id =
                capture_stdout_event_session_id(&stdout, event_type, event_id_path);
            Ok(json!({
                "provider_session_id": provider_session_id,
                "state": {
                    "capture_kind": "stdout_json_event",
                    "event_type": event_type,
                    "event_id_path": event_id_path,
                    "found": provider_session_id.is_some(),
                },
                "artifacts": [],
            }))
        }
        other => Err(ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_capture_kind",
            format!("unsupported session capture kind: {other}"),
        )),
    }
}

fn capture_stdout_event_session_id(
    stdout: &[u8],
    event_type: &str,
    event_id_path: &str,
) -> Option<String> {
    let text = std::str::from_utf8(stdout).ok()?;
    for line in text.lines() {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        if value.get("type").and_then(Value::as_str) != Some(event_type) {
            continue;
        }
        if let Some(id) = json_path_string(&value, event_id_path) {
            return Some(id.to_string());
        }
    }
    None
}

fn json_path_string<'a>(value: &'a Value, path: &str) -> Option<&'a str> {
    path.split('.')
        .try_fold(value, |current, part| current.get(part))
        .and_then(Value::as_str)
}

fn canonical_records_from_claude_file(
    request_id: &str,
    path: &Path,
    session_id: &str,
    provider_name: &str,
) -> Result<Vec<CanonicalRecord>, ProviderFailure> {
    let lines = scan_jsonl_file(request_id, path)?;
    let mut records = Vec::new();
    let mut latest_compaction_boundary = None;
    for line in lines {
        let Some(recorded_session_id) = line.value.get("sessionId").and_then(Value::as_str) else {
            continue;
        };
        if recorded_session_id != session_id {
            return Err(ProviderFailure::unavailable(
                request_id.to_string(),
                "malformed_transcript",
                format!("transcript sessionId {recorded_session_id} does not match requested session {session_id}"),
                false,
                json!({ "path": path.display().to_string(), "line": line.line }),
            ));
        }
        let Some(native_type) = line.value.get("type").and_then(Value::as_str) else {
            continue;
        };
        let turn_id = required_json_string(request_id, path, &line, "uuid")?;
        let timestamp = required_json_string(request_id, path, &line, "timestamp")?;
        validate_rfc3339(request_id, path, line.line, &timestamp)?;
        let unsupported_record = !matches!(native_type, "user" | "assistant");
        let content = if unsupported_record {
            Vec::new()
        } else {
            extract_claude_content(&line.value)
        };
        let record = CanonicalRecord {
            session_id: session_id.to_string(),
            provider_name: provider_name.to_string(),
            turn_id,
            role: native_type.to_string(),
            timestamp,
            content,
            source: RecordSource {
                storage_type: "claude_code".to_string(),
                jsonl_path: path.to_path_buf(),
                line: line.line,
                byte_start: line.byte_start,
                byte_end: line.byte_end,
                sha256: line.sha256,
            },
            unsupported_record,
        };
        if line
            .value
            .get("isCompactSummary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            latest_compaction_boundary = Some(records.len());
        }
        records.push(record);
    }
    if let Some(index) = latest_compaction_boundary {
        records = records.into_iter().skip(index).collect();
    }
    validate_record_order(request_id, path, &records)?;
    Ok(records)
}

fn scan_jsonl_file(request_id: &str, path: &Path) -> Result<Vec<SourceLine>, ProviderFailure> {
    let bytes = fs::read(path).map_err(|err| {
        ProviderFailure::unavailable(
            request_id.to_string(),
            "transcript_read_failed",
            format!("failed to read transcript {}: {err}", path.display()),
            false,
            json!({ "path": path.display().to_string() }),
        )
    })?;
    scan_jsonl_bytes(request_id, path, &bytes)
}

fn scan_jsonl_bytes(
    request_id: &str,
    path: &Path,
    bytes: &[u8],
) -> Result<Vec<SourceLine>, ProviderFailure> {
    let mut out = Vec::new();
    let mut line = 1_u64;
    let mut offset = 0_usize;
    while offset < bytes.len() {
        let start = offset;
        while offset < bytes.len() && bytes[offset] != b'\n' {
            offset += 1;
        }
        let mut end = offset;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        let line_bytes = &bytes[start..end];
        if !line_bytes.iter().all(u8::is_ascii_whitespace) {
            let text = std::str::from_utf8(line_bytes).map_err(|err| {
                malformed_transcript(request_id, path, line, format!("line is not UTF-8: {err}"))
            })?;
            let value = serde_json::from_str::<Value>(text).map_err(|err| {
                malformed_transcript(
                    request_id,
                    path,
                    line,
                    format!("line is not valid JSON: {err}"),
                )
            })?;
            out.push(SourceLine {
                line,
                byte_start: start as u64,
                byte_end: end as u64,
                sha256: sha256_hex(line_bytes),
                value,
            });
        }
        if offset < bytes.len() && bytes[offset] == b'\n' {
            offset += 1;
        }
        line += 1;
    }
    Ok(out)
}

fn required_json_string(
    request_id: &str,
    path: &Path,
    line: &SourceLine,
    field: &str,
) -> Result<String, ProviderFailure> {
    line.value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            malformed_transcript(
                request_id,
                path,
                line.line,
                format!("transcript line is missing required {field}"),
            )
        })
}

fn validate_rfc3339(
    request_id: &str,
    path: &Path,
    line: u64,
    timestamp: &str,
) -> Result<(), ProviderFailure> {
    DateTime::parse_from_rfc3339(timestamp)
        .map(|_| ())
        .map_err(|err| {
            malformed_transcript(
                request_id,
                path,
                line,
                format!("transcript timestamp is not RFC3339: {err}"),
            )
        })
}

fn validate_record_order(
    request_id: &str,
    path: &Path,
    records: &[CanonicalRecord],
) -> Result<(), ProviderFailure> {
    let mut previous: Option<DateTime<Utc>> = None;
    for record in records {
        let current = DateTime::parse_from_rfc3339(&record.timestamp)
            .map(|timestamp| timestamp.with_timezone(&Utc))
            .map_err(|err| {
                malformed_transcript(
                    request_id,
                    path,
                    record.source.line,
                    format!("transcript timestamp is not RFC3339: {err}"),
                )
            })?;
        if previous.is_some_and(|previous| current < previous) {
            return Err(malformed_transcript(
                request_id,
                path,
                record.source.line,
                "transcript timestamps are not in provider order".to_string(),
            ));
        }
        previous = Some(current);
    }
    Ok(())
}

fn malformed_transcript(
    request_id: &str,
    path: &Path,
    line: u64,
    reason: String,
) -> ProviderFailure {
    ProviderFailure::unavailable(
        request_id.to_string(),
        "malformed_transcript",
        reason,
        false,
        json!({ "path": path.display().to_string(), "line": line }),
    )
}

fn extract_claude_content(value: &Value) -> Vec<ContentChunk> {
    if let Some(message) = value.get("message") {
        if let Some(text) = message.as_str() {
            return vec![text_chunk(text)];
        }
        if let Some(content) = message.get("content") {
            return extract_content_chunks(Some(content));
        }
    }
    extract_content_chunks(value.get("content"))
}

fn extract_claude_body_json(value: &Value) -> Vec<Value> {
    extract_claude_content(value)
        .into_iter()
        .map(|chunk| json!({ "type": chunk.chunk_type, "text": chunk.text.unwrap_or_default() }))
        .collect()
}

fn extract_content_chunks(value: Option<&Value>) -> Vec<ContentChunk> {
    match value {
        Some(Value::String(text)) => vec![text_chunk(text)],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    return Some(text_chunk(text));
                }
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("text");
                let text = item
                    .get("text")
                    .or_else(|| item.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                text.map(|text| ContentChunk {
                    chunk_type: canonical_chunk_type(item_type).to_string(),
                    text: Some(text),
                })
            })
            .collect(),
        Some(Value::Object(object)) => object
            .get("text")
            .or_else(|| object.get("content"))
            .and_then(Value::as_str)
            .map(|text| vec![text_chunk(text)])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn canonical_chunk_type(native_type: &str) -> &str {
    match native_type {
        "input_text" | "output_text" => "text",
        other => other,
    }
}

fn text_chunk(text: &str) -> ContentChunk {
    ContentChunk {
        chunk_type: "text".to_string(),
        text: Some(text.to_string()),
    }
}

fn canonical_jsonl_bytes(
    request_id: &str,
    records: &[CanonicalRecord],
) -> Result<Vec<u8>, ProviderFailure> {
    let mut bytes = Vec::new();
    for record in records {
        let line = serde_json::to_string(record).map_err(|err| {
            ProviderFailure::unavailable(
                request_id.to_string(),
                "canonical_serialize_failed",
                format!("failed to serialize canonical record: {err}"),
                false,
                json!({}),
            )
        })?;
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn parse_canonical_jsonl(
    request_id: &str,
    bytes: &[u8],
) -> Result<Vec<CanonicalRecord>, ProviderFailure> {
    let text = std::str::from_utf8(bytes).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_canonical_utf8",
            format!("canonical transcript is not UTF-8: {err}"),
        )
    })?;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index as u64 + 1;
        if line.trim().is_empty() {
            return Err(ProviderFailure::invalid_request(
                request_id.to_string(),
                "invalid_canonical_transcript",
                "blank line in canonical JSONL",
            ));
        }
        let record: CanonicalRecord = serde_json::from_str(line).map_err(|err| {
            ProviderFailure::invalid_request(
                request_id.to_string(),
                "invalid_canonical_transcript",
                format!("malformed canonical JSONL line {line_no}: {err}"),
            )
        })?;
        validate_canonical_record_shape(request_id, line_no, &record)?;
        records.push(record);
    }
    if records.is_empty() {
        return Err(ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_canonical_transcript",
            "empty canonical transcript",
        ));
    }
    if records.iter().all(|record| record.unsupported_record) {
        return Err(ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_canonical_transcript",
            "canonical transcript has no replaceable records",
        ));
    }
    Ok(records)
}

fn validate_canonical_record_shape(
    request_id: &str,
    line: u64,
    record: &CanonicalRecord,
) -> Result<(), ProviderFailure> {
    if record.session_id.is_empty()
        || record.provider_name.is_empty()
        || record.turn_id.is_empty()
        || record.timestamp.is_empty()
        || (!record.unsupported_record && !matches!(record.role.as_str(), "user" | "assistant"))
    {
        return Err(ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_canonical_transcript",
            format!("canonical record line {line} is missing required fields"),
        ));
    }
    DateTime::parse_from_rfc3339(&record.timestamp).map_err(|err| {
        ProviderFailure::invalid_request(
            request_id.to_string(),
            "invalid_canonical_transcript",
            format!("invalid canonical timestamp on line {line}: {err}"),
        )
    })?;
    Ok(())
}

fn validate_replacement_records(
    request_id: &str,
    records: &[CanonicalRecord],
    session_id: &str,
    provider_name: &str,
) -> Result<(), ProviderFailure> {
    for (index, record) in records.iter().enumerate() {
        if record.session_id != session_id || record.provider_name != provider_name {
            return Err(ProviderFailure::invalid_request(
                request_id.to_string(),
                "replacement_target_mismatch",
                format!(
                    "canonical record line {} does not match target session/provider",
                    index + 1
                ),
            ));
        }
        if record.unsupported_record {
            return Err(ProviderFailure::invalid_request(
                request_id.to_string(),
                "replacement_unsupported_record",
                "unsupported canonical records cannot be rendered into Claude storage",
            ));
        }
        for chunk in &record.content {
            if chunk.text.is_none() {
                return Err(ProviderFailure::invalid_request(
                    request_id.to_string(),
                    "replacement_unsupported_content",
                    format!(
                        "content chunk type {} cannot be rendered without text",
                        chunk.chunk_type
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn render_claude_records(
    request_id: &str,
    records: &[CanonicalRecord],
) -> Result<Vec<u8>, ProviderFailure> {
    let mut bytes = Vec::new();
    for record in records {
        let content = record
            .content
            .iter()
            .map(|chunk| json!({ "type": chunk.chunk_type, "text": chunk.text.as_deref().unwrap_or("") }))
            .collect::<Vec<_>>();
        let line = json!({
            "type": record.role,
            "uuid": record.turn_id,
            "sessionId": record.session_id,
            "timestamp": record.timestamp,
            "message": {
                "role": record.role,
                "content": content,
            },
        });
        let line = serde_json::to_string(&line).map_err(|err| {
            ProviderFailure::unavailable(
                request_id.to_string(),
                "render_failed",
                format!("failed to render Claude transcript line: {err}"),
                false,
                json!({}),
            )
        })?;
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn canonical_semantics_equal(left: &[CanonicalRecord], right: &[CanonicalRecord]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.session_id == right.session_id
                && left.provider_name == right.provider_name
                && left.turn_id == right.turn_id
                && left.role == right.role
                && left.timestamp == right.timestamp
                && left.unsupported_record == right.unsupported_record
                && content_chunks_equal(&left.content, &right.content)
        })
}

fn content_chunks_equal(left: &[ContentChunk], right: &[ContentChunk]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.chunk_type == right.chunk_type && left.text == right.text)
}

fn atomic_replace_file(request_id: &str, path: &Path, bytes: &[u8]) -> Result<(), ProviderFailure> {
    let parent = path.parent().ok_or_else(|| {
        ProviderFailure::unavailable(
            request_id.to_string(),
            "replace_path_invalid",
            "transcript path has no parent directory",
            false,
            json!({ "path": path.display().to_string() }),
        )
    })?;
    let tmp_path = path.with_extension(format!("jsonl.tmp-session-replace-{}", now_unix_ms()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|err| {
                ProviderFailure::unavailable(
                    request_id.to_string(),
                    "replace_tmp_write_failed",
                    format!(
                        "failed to create replacement temp file {}: {err}",
                        tmp_path.display()
                    ),
                    false,
                    json!({ "path": tmp_path.display().to_string() }),
                )
            })?;
        file.write_all(bytes).map_err(|err| {
            ProviderFailure::unavailable(
                request_id.to_string(),
                "replace_tmp_write_failed",
                format!(
                    "failed to write replacement temp file {}: {err}",
                    tmp_path.display()
                ),
                false,
                json!({ "path": tmp_path.display().to_string() }),
            )
        })?;
        file.sync_all().map_err(|err| {
            ProviderFailure::unavailable(
                request_id.to_string(),
                "replace_tmp_sync_failed",
                format!(
                    "failed to sync replacement temp file {}: {err}",
                    tmp_path.display()
                ),
                false,
                json!({ "path": tmp_path.display().to_string() }),
            )
        })?;
    }
    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(ProviderFailure::unavailable(
            request_id.to_string(),
            "replace_conflict",
            format!(
                "failed to atomically replace transcript {}: {err}",
                path.display()
            ),
            true,
            json!({ "path": path.display().to_string() }),
        ));
    }
    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

fn session_artifact(path: &Path, sha256: &str) -> Value {
    json!({
        "kind": "file",
        "path": path.display().to_string(),
        "sha256": sha256,
    })
}

fn session_context_string(params: &SessionParams, key: &str) -> Option<String> {
    params
        .context
        .as_ref()
        .and_then(|value| nested_context_value(value, key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn legacy_session_quota_settings(
    config_root: &str,
    candidates: &[String],
) -> Option<ProviderQuotaSettings> {
    let sessions_toml = std::path::Path::new(config_root).join("sessions.toml");
    let text = std::fs::read_to_string(sessions_toml).ok()?;
    parse_provider_quota_settings(&text, candidates)
}

fn provider_config_candidates(
    context: &Option<Value>,
    provider_instance_id: Option<&str>,
    settings_id: &str,
) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(provider_name) = context_string(context, "provider_name") {
        push_candidate(&mut candidates, provider_name);
    }
    if let Some(provider_instance_id) = provider_instance_id {
        push_candidate(&mut candidates, provider_instance_id.to_string());
    }
    push_candidate(&mut candidates, settings_id.to_string());
    candidates
}

fn push_candidate(candidates: &mut Vec<String>, value: String) {
    if value.trim().is_empty() || candidates.iter().any(|candidate| candidate == &value) {
        return;
    }
    candidates.push(value);
}

fn parse_provider_quota_settings(
    providers_toml: &str,
    candidates: &[String],
) -> Option<ProviderQuotaSettings> {
    let parsed: toml::Value = toml::from_str(providers_toml).ok()?;
    candidates.iter().find_map(|candidate| {
        let settings = parse_provider_quota_settings_for_candidate(&parsed, candidate);
        active_has_quota(&settings).then_some(settings)
    })
}

fn parse_provider_quota_settings_for_candidate(
    providers_toml: &toml::Value,
    candidate: &str,
) -> ProviderQuotaSettings {
    let mut active = ProviderQuotaSettings::default();
    let Some(table) = providers_toml.get(candidate) else {
        return active;
    };
    active.quota_script = table_string(table, "quota_script");
    active.auth_refresh_command = table_string(table, "auth_refresh_command");
    if active.quota_script.is_none() {
        active.quota_script = table_string(table, "turn_script")
            .or_else(|| table_string(table, "cwd_script"))
            .and_then(|command| derived_quota_script_from_adapter_command(&command));
    }
    if active.quota_script.is_none() {
        active.quota_script = table
            .get("session_storage")
            .and_then(derived_quota_script_from_session_storage);
    }
    active
}

fn active_has_quota(settings: &ProviderQuotaSettings) -> bool {
    settings.quota_script.is_some() || settings.auth_refresh_command.is_some()
}

fn table_string(table: &toml::Value, key: &str) -> Option<String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
}

fn derived_quota_script_from_session_storage(storage: &toml::Value) -> Option<String> {
    table_string(storage, "cwd_script")
        .and_then(|command| derived_quota_script_from_adapter_command(&command))
        .or_else(|| derived_quota_script_from_claude_code_storage(storage))
}

fn derived_quota_script_from_claude_code_storage(storage: &toml::Value) -> Option<String> {
    if table_string(storage, "kind").as_deref() != Some("claude_code") {
        return None;
    }
    let projects_dir = table_string(storage, "projects_dir")?;
    let cwd_script = format!("claude-code-cwd {}", shell_word_arg(&projects_dir));
    derived_quota_script_from_adapter_command(&cwd_script)
}

fn derived_quota_script_from_adapter_command(command: &str) -> Option<String> {
    let parts = shell_split(command);
    let adapter = parts.first()?;
    let adapter_name = std::path::Path::new(adapter).file_name()?.to_str()?;
    let storage_root = parts.get(1)?;
    let account_root = std::path::Path::new(storage_root).parent()?;
    match adapter_name {
        "claude-code-turns" | "claude-code-cwd" => Some(format!(
            "anthropic-usage {}",
            shell_word_arg(&account_root.join(".credentials.json").to_string_lossy())
        )),
        _ => None,
    }
}

fn shell_word_arg(input: &str) -> String {
    if is_bare_shell_word(input) {
        return input.to_string();
    }
    quote_shell_word(input)
}

fn is_bare_shell_word(input: &str) -> bool {
    input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
}

fn quote_shell_word(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

fn context_string(context: &Option<Value>, key: &str) -> Option<String> {
    context
        .as_ref()
        .and_then(|value| nested_context_value(value, key))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn context_u64(context: &Option<Value>, key: &str) -> Option<u64> {
    context
        .as_ref()
        .and_then(|value| nested_context_value(value, key))
        .and_then(Value::as_u64)
}

fn nested_context_value<'a>(context: &'a Value, key: &str) -> Option<&'a Value> {
    context
        .get(key)
        .or_else(|| {
            context
                .get("settings")
                .and_then(|settings| settings.get(key))
        })
        .or_else(|| context.get("cache").and_then(|cache| cache.get(key)))
}

fn quota_source_freshness(has_source: bool, context: &Option<Value>) -> &'static str {
    if !has_source {
        return "no_source";
    }
    if cached_quota_fresh(context) {
        return "fresh";
    }
    "probe_required"
}

fn cached_quota_fresh(context: &Option<Value>) -> bool {
    let Some(checked_at) = context_u64(context, "cached_checked_at_unix_ms") else {
        return false;
    };
    let windows = cached_windows(context);
    if windows.is_empty() {
        return false;
    }
    let now = context_u64(context, "now_unix_ms").unwrap_or_else(now_unix_ms);
    let ttl_ms = dynamic_quota_ttl_ms(&windows, now);
    now.saturating_sub(checked_at) < ttl_ms
}

fn cached_windows(context: &Option<Value>) -> Vec<CachedQuotaWindow> {
    let Some(value) = context
        .as_ref()
        .and_then(|item| nested_context_value(item, "cached_windows"))
    else {
        return Vec::new();
    };
    serde_json::from_value(value.clone()).unwrap_or_default()
}

fn context_has_prior_windows(context: &Option<Value>) -> bool {
    if context
        .as_ref()
        .and_then(|item| nested_context_value(item, "had_prior_windows"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    !cached_windows(context).is_empty()
}

fn dynamic_quota_ttl_ms(windows: &[CachedQuotaWindow], now_unix_ms: u64) -> u64 {
    const MIN_TTL_MS: u64 = 5 * 60 * 1000;
    const MAX_TTL_MS: u64 = 24 * 60 * 60 * 1000;
    const REFRESH_WINDOW_DIVISOR: u64 = 5;
    let min_reset_ms = windows
        .iter()
        .map(|window| window.resets_at_unix_ms.saturating_sub(now_unix_ms))
        .min()
        .unwrap_or(MAX_TTL_MS);
    (min_reset_ms / REFRESH_WINDOW_DIVISOR).clamp(MIN_TTL_MS, MAX_TTL_MS)
}

fn parse_quota_script_output(stdout: &str) -> Result<Vec<Value>, String> {
    let parsed: QuotaScriptOutput = serde_json::from_str(stdout.trim()).map_err(|err| {
        format!(
            "Invalid JSON from quota script: {err} (got: {})",
            stdout.trim()
        )
    })?;
    let mut windows = match parsed.windows {
        Some(windows) => windows,
        None => legacy_quota_window(parsed.used_percent, parsed.resets_at, stdout)?,
    };
    let mut output = Vec::with_capacity(windows.len());
    for (index, window) in windows.iter_mut().enumerate() {
        window.window_id = index as u32;
        output.push(quota_window_json(window, stdout)?);
    }
    Ok(output)
}

fn legacy_quota_window(
    used_percent: Option<f64>,
    resets_at: Option<String>,
    stdout: &str,
) -> Result<Vec<QuotaScriptWindow>, String> {
    let Some(used_percent) = used_percent else {
        return Err(format!(
            "quota script emitted neither `windows` nor `used_percent` (got: {})",
            stdout.trim()
        ));
    };
    let Some(resets_at) = resets_at else {
        return Err(format!(
            "legacy quota script emitted `used_percent` without `resets_at` (got: {})",
            stdout.trim()
        ));
    };
    Ok(vec![QuotaScriptWindow {
        window_id: 0,
        used_percent,
        resets_at,
        label: None,
    }])
}

fn quota_window_json(window: &QuotaScriptWindow, stdout: &str) -> Result<Value, String> {
    validate_used_percent(window.used_percent, stdout)?;
    let resets_at_unix_ms = parse_rfc3339_unix_ms(&window.resets_at)?;
    let mut value = json!({
        "remaining_ratio": 1.0 - (window.used_percent / 100.0),
        "resets_at_unix_ms": resets_at_unix_ms,
    });
    if let Some(label) = window.label.as_deref().filter(|label| !label.is_empty()) {
        value["name"] = json!(label);
    }
    Ok(value)
}

fn validate_used_percent(used_percent: f64, stdout: &str) -> Result<(), String> {
    if !used_percent.is_nan() && (0.0..=100.0).contains(&used_percent) {
        return Ok(());
    }
    Err(format!(
        "quota script emitted used_percent={used_percent} outside 0..100 (got: {})",
        stdout.trim()
    ))
}

fn parse_rfc3339_unix_ms(timestamp: &str) -> Result<u64, String> {
    let parsed = DateTime::parse_from_rfc3339(timestamp)
        .map_err(|err| format!("Bad resets_at {timestamp}: {err}"))?
        .with_timezone(&Utc);
    u64::try_from(parsed.timestamp_millis())
        .map_err(|_| format!("Bad resets_at {timestamp}: timestamp before unix epoch"))
}

fn quota_probe_detail(windows: &[Value]) -> String {
    if windows.is_empty() {
        return "no quota windows reported".to_string();
    }
    format!("{} quota window(s) reported", windows.len())
}

#[derive(Clone, Copy)]
enum CommandKind {
    Quota,
    Auth,
}

struct ShellCommandOutput {
    stdout: String,
}

enum ShellCommandFailure {
    Spawn(String),
    Wait(String),
    Timeout(CommandKind),
    Nonzero { code: i32, stderr: String },
}

fn run_shell_command(
    command: &str,
    timeout: Duration,
    kind: CommandKind,
) -> Result<String, ShellCommandFailure> {
    let mut child = Command::new("sh");
    child.arg("-c").arg(command);
    child.stdin(Stdio::null());
    child.stdout(Stdio::piped());
    child.stderr(Stdio::piped());
    let mut child = child
        .spawn()
        .map_err(|err| ShellCommandFailure::Spawn(format_command_spawn_error(kind, err)))?;
    let stdout = child
        .stdout
        .take()
        .map(spawn_string_drain)
        .expect("stdout was piped");
    let stderr = child
        .stderr
        .take()
        .map(spawn_string_drain)
        .expect("stderr was piped");
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = ShellCommandOutput {
                    stdout: stdout.join().unwrap_or_default(),
                };
                let stderr = stderr.join().unwrap_or_default();
                if status.success() {
                    return Ok(output.stdout);
                }
                return Err(ShellCommandFailure::Nonzero {
                    code: status.code().unwrap_or(-1),
                    stderr,
                });
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ShellCommandFailure::Timeout(kind));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                return Err(ShellCommandFailure::Wait(format_command_wait_error(
                    kind, err,
                )))
            }
        }
    }
}

fn spawn_string_drain<R>(mut reader: R) -> thread::JoinHandle<String>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = String::new();
        let _ = reader.read_to_string(&mut buffer);
        buffer
    })
}

fn command_failure(
    request_id: &str,
    failure: ShellCommandFailure,
    default_code: &'static str,
) -> ProviderFailure {
    match failure {
        ShellCommandFailure::Spawn(message) | ShellCommandFailure::Wait(message) => {
            ProviderFailure::unavailable(
                request_id.to_string(),
                default_code,
                message,
                true,
                json!({ "refresh_auth_recommended": true }),
            )
        }
        ShellCommandFailure::Timeout(kind) => timeout_failure(request_id, kind),
        ShellCommandFailure::Nonzero { code, stderr } => ProviderFailure::unavailable(
            request_id.to_string(),
            default_code,
            format!("quota command exited {code}: {}", stderr.trim()),
            true,
            json!({
                "exit_code": code,
                "stderr": stderr,
                "refresh_auth_recommended": true,
            }),
        ),
    }
}

fn timeout_failure(request_id: &str, kind: CommandKind) -> ProviderFailure {
    let (code, message) = match kind {
        CommandKind::Quota => ("quota_probe_timeout", "quota script timed out"),
        CommandKind::Auth => (
            "quota_refresh_auth_timeout",
            "auth_refresh_command timed out",
        ),
    };
    ProviderFailure {
        request_id: request_id.to_string(),
        code,
        category: "timeout",
        message: message.to_string(),
        retryable: true,
        details: json!({ "refresh_auth_recommended": true }),
        exit_code: 1,
    }
}

fn format_command_spawn_error(kind: CommandKind, error: std::io::Error) -> String {
    match kind {
        CommandKind::Quota => format!("Failed to spawn quota script: {error}"),
        CommandKind::Auth => format!("Failed to spawn auth_refresh_command: {error}"),
    }
}

fn format_command_wait_error(kind: CommandKind, error: std::io::Error) -> String {
    match kind {
        CommandKind::Quota => format!("Quota script wait failed: {error}"),
        CommandKind::Auth => format!("auth_refresh_command wait failed: {error}"),
    }
}

fn diagnostic(severity: &str, message: &str, code: &str) -> Value {
    json!({
        "severity": severity,
        "message": message,
        "code": code,
    })
}

fn flag_values<'a>(argv: &'a [String], flags: &[&str]) -> Vec<(String, &'a str)> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let token = argv[i].as_str();
        if flags.contains(&token) {
            if let Some(value) = argv.get(i + 1) {
                values.push((token.to_string(), value.as_str()));
            }
        } else if let Some((flag, value)) = token.split_once('=') {
            if flags.contains(&flag) {
                values.push((flag.to_string(), value));
            }
        }
        i += 1;
    }
    values
}

fn shell_split(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;
    for ch in command.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if escape {
        current.push('\\');
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn byte_payload_bytes(payload: &BytePayload) -> Result<Vec<u8>, String> {
    match payload.encoding.as_str() {
        "utf8" => Ok(payload.data.as_bytes().to_vec()),
        "base64" => decode_base64(&payload.data),
        other => Err(format!("unsupported byte payload encoding: {other}")),
    }
}

fn process_status_from_output(status: &std::process::ExitStatus) -> ProcessStatus {
    if let Some(code) = status.code() {
        return ProcessStatus::Exited { code };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ProcessStatus::SignalTerminated { signal };
        }
    }
    ProcessStatus::Unknown
}

fn classify_terminal_signal(
    stdout: &[u8],
    stderr: &[u8],
    status: &ProcessStatus,
    observed_at_unix_ms: u64,
) -> TerminalSignal {
    let (kind, evidence) = match status {
        ProcessStatus::SpawnError { reason } => (TerminalSignalKind::SpawnError, reason.clone()),
        ProcessStatus::ProlongedSilence { reason } => {
            (TerminalSignalKind::ProlongedSilence, reason.clone())
        }
        ProcessStatus::SignalTerminated { signal } => {
            (TerminalSignalKind::SignalExit, format!("signal={signal}"))
        }
        ProcessStatus::Cancelled => (TerminalSignalKind::Cancelled, "cancelled".to_string()),
        ProcessStatus::Exited { code: 0 } => {
            if contains_persistent_quota_token(stdout, stderr) {
                (
                    TerminalSignalKind::QuotaExhaustedInband,
                    quota_evidence(stdout, stderr),
                )
            } else if contains_transient_rate_limit_token(stdout, stderr) {
                (
                    TerminalSignalKind::RateLimited,
                    quota_evidence(stdout, stderr),
                )
            } else {
                (TerminalSignalKind::CleanExit, "exit_code=0".to_string())
            }
        }
        ProcessStatus::Exited { code } => {
            if contains_persistent_quota_token(stdout, stderr) {
                (
                    TerminalSignalKind::QuotaExhaustedInband,
                    quota_evidence(stdout, stderr),
                )
            } else if contains_transient_rate_limit_token(stdout, stderr) {
                (
                    TerminalSignalKind::RateLimited,
                    quota_evidence(stdout, stderr),
                )
            } else {
                (TerminalSignalKind::NonzeroExit, format!("exit_code={code}"))
            }
        }
        ProcessStatus::Unknown => (TerminalSignalKind::Unknown, "unknown".to_string()),
    };
    TerminalSignal {
        kind,
        evidence: Some(bounded_text(&evidence, 160)),
        observed_at_unix_ms,
    }
}

fn contains_persistent_quota_token(_stdout: &[u8], _stderr: &[u8]) -> bool {
    false
}

fn contains_transient_rate_limit_token(_stdout: &[u8], _stderr: &[u8]) -> bool {
    false
}

fn quota_evidence(stdout: &[u8], stderr: &[u8]) -> String {
    if !stdout.is_empty() {
        return String::from_utf8_lossy(stdout).into_owned();
    }
    String::from_utf8_lossy(stderr).into_owned()
}

fn terminal_signal_json(signal: &TerminalSignal) -> Value {
    json!({
        "kind": terminal_signal_kind_str(signal.kind),
        "evidence": signal.evidence,
        "observed_at_unix_ms": signal.observed_at_unix_ms,
    })
}

fn terminal_signal_kind_str(kind: TerminalSignalKind) -> &'static str {
    match kind {
        TerminalSignalKind::CleanExit => "clean_exit",
        TerminalSignalKind::NonzeroExit => "nonzero_exit",
        TerminalSignalKind::SignalExit => "signal_exit",
        TerminalSignalKind::SpawnError => "spawn_error",
        TerminalSignalKind::QuotaExhaustedInband => "quota_exhausted_inband",
        TerminalSignalKind::MaybeQuotaExhausted => "maybe_quota_exhausted",
        TerminalSignalKind::RateLimited => "rate_limited",
        TerminalSignalKind::ProlongedSilence => "prolonged_silence",
        TerminalSignalKind::Cancelled => "cancelled",
        TerminalSignalKind::Unknown => "unknown",
    }
}

fn bounded_text(text: &str, max_len: usize) -> String {
    text.chars().take(max_len).collect()
}

struct LaunchStream<'a, W: Write> {
    request_id: &'a str,
    writer: &'a mut W,
    seq: u64,
}

impl<'a, W: Write> LaunchStream<'a, W> {
    fn new(request_id: &'a str, writer: &'a mut W) -> Self {
        Self {
            request_id,
            writer,
            seq: 0,
        }
    }

    fn bytes(&mut self, kind: &str, bytes: &[u8]) {
        self.seq += 1;
        self.write_event(json!({
            "contract": CONTRACT,
            "request_id": self.request_id,
            "seq": self.seq,
            "time_unix_ms": now_unix_ms(),
            "kind": kind,
            "data_base64": encode_base64(bytes),
        }));
    }

    fn marker(&mut self, name: &str) {
        self.seq += 1;
        self.write_event(json!({
            "contract": CONTRACT,
            "request_id": self.request_id,
            "seq": self.seq,
            "time_unix_ms": now_unix_ms(),
            "kind": "marker",
            "name": name,
            "value": true,
        }));
    }

    fn exit(
        &mut self,
        status: ProcessStatus,
        terminal_signal: TerminalSignal,
        session: Option<Value>,
    ) {
        self.seq += 1;
        let mut event = json!({
            "contract": CONTRACT,
            "request_id": self.request_id,
            "seq": self.seq,
            "time_unix_ms": now_unix_ms(),
            "kind": "exit",
            "status": status,
            "terminal_signal": terminal_signal_json(&terminal_signal),
        });
        if let Some(session) = session {
            event["session"] = session;
        }
        self.write_event(event);
    }

    fn write_event(&mut self, event: Value) {
        let _ = writeln!(self.writer, "{event}");
        let _ = self.writer.flush();
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let clean = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if clean.len() % 4 != 0 {
        return Err("base64 length must be a multiple of four".to_string());
    }
    let mut bytes = Vec::new();
    for chunk in clean.chunks(4) {
        let mut values = [0u8; 4];
        let mut padding = 0;
        for (index, byte) in chunk.iter().copied().enumerate() {
            if byte == b'=' {
                values[index] = 0;
                padding += 1;
            } else {
                values[index] = base64_value(byte)?;
            }
        }
        bytes.push((values[0] << 2) | (values[1] >> 4));
        if padding < 2 {
            bytes.push((values[1] << 4) | (values[2] >> 2));
        }
        if padding == 0 {
            bytes.push((values[2] << 6) | values[3]);
        }
    }
    Ok(bytes)
}

fn base64_value(byte: u8) -> Result<u8, String> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => Err(format!("invalid base64 byte 0x{byte:02x}")),
    }
}

fn error_response(failure: &ProviderFailure) -> String {
    let response = json!({
        "contract": CONTRACT,
        "request_id": failure.request_id,
        "ok": false,
        "error": {
            "code": failure.code,
            "category": failure.category,
            "message": failure.message,
            "retryable": failure.retryable,
            "details": failure.details,
        },
    });
    serde_json::to_string(&response).expect("error serialization is infallible")
}

pub fn describe_result() -> Value {
    json!({
        "provider_id": "claude",
        "display_name": "Claude Code",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": true,
            "policy": true,
            "quota": true,
            "session": true,
            "terminal": true,
            "rotation": false,
            "discovery": false,
            "settings": false,
            "setup_brain": false,
            "setup": false,
            "migration": false,
        },
        "settings_schema_id": SETTINGS_SCHEMA_ID,
        "concurrency": {
            "safe_for_parallel_invocation": true,
            "state_locking": "atomic_file_writes_and_provider_cli_owned_state",
            "settings_version_tokens": true,
            "stdout_protocol_only": true,
            "notes": "This provider is one-shot and daemonless; future settings mutations must use version tokens and atomic persistence.",
        },
    })
}

pub fn settings_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SETTINGS_SCHEMA_URI,
        "title": "Claude Provider Settings",
        "type": "object",
        "required": ["command"],
        "additionalProperties": false,
        "properties": {
            "name": {
                "type": "string",
                "minLength": 1,
                "description": "Stable provider account identifier. When omitted during migration, agent-runner derives it from command and args."
            },
            "command": {
                "type": "string",
                "minLength": 1,
                "default": "env -u CLAUDECODE claude",
                "description": "Base Claude executable command, including any env prefix."
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "default": []
            },
            "interactive_args": {
                "type": "array",
                "items": { "type": "string" }
            },
            "prompt_mode": {
                "type": "string",
                "enum": ["stdin", "arg"],
                "default": "stdin"
            },
            "invocation_mode": {
                "type": "string",
                "enum": ["headless", "proxy"],
                "default": "headless"
            },
            "quota_script": {
                "type": "string",
                "description": "Command that emits Claude quota windows, commonly anthropic-usage against a Claude credentials file."
            },
            "auth_refresh_command": {
                "type": "string",
                "default": "claude auth status"
            },
            "resume": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind", "flag"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "flag" },
                            "flag": { "type": "string", "minLength": 1 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "subcommand"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "subcommand" },
                            "subcommand": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1 },
                                "minItems": 1
                            }
                        }
                    }
                ]
            },
            "session_capture": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "none" }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "flag"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "forced_flag_verified" },
                            "flag": { "type": "string", "minLength": 1 },
                            "readback_args": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        }
                    },
                    {
                        "type": "object",
                        "required": [
                            "kind",
                            "json_flag",
                            "last_message_flag",
                            "event_type",
                            "event_id_path"
                        ],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "stdout_json_event" },
                            "json_flag": { "type": "string", "minLength": 1 },
                            "last_message_flag": { "type": "string", "minLength": 1 },
                            "event_type": { "type": "string", "minLength": 1 },
                            "event_id_path": { "type": "string", "minLength": 1 }
                        }
                    }
                ]
            },
            "resume_acceptance": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "accepted_output_patterns": {
                        "type": "array",
                        "items": { "type": "string", "minLength": 1 }
                    },
                    "rejected_output_patterns": {
                        "type": "array",
                        "items": { "type": "string", "minLength": 1 }
                    }
                }
            },
            "session_storage": {
                "oneOf": [
                    {
                        "type": "object",
                        "required": ["kind", "projects_dir"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "claude_code" },
                            "projects_dir": { "type": "string", "minLength": 1 }
                        }
                    },
                    {
                        "type": "object",
                        "required": ["kind", "cwd_script"],
                        "additionalProperties": false,
                        "properties": {
                            "kind": { "const": "script" },
                            "cwd_script": { "type": "string", "minLength": 1 },
                            "transcript_script": { "type": "string", "minLength": 1 },
                            "storage_type": { "const": "claude_code" }
                        },
                        "dependentRequired": {
                            "transcript_script": ["storage_type"],
                            "storage_type": ["transcript_script"]
                        }
                    }
                ]
            },
            "system_prompt_override": {
                "type": "string"
            },
            "tool_restrictions": {
                "type": "object",
                "required": ["kind"],
                "additionalProperties": false,
                "properties": {
                    "kind": { "const": "claude" },
                    "claude": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "disallowed_tools": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1 }
                            },
                            "allowed_tools": {
                                "type": "array",
                                "items": { "type": "string", "minLength": 1 }
                            },
                            "disable_slash_commands": {
                                "type": "boolean",
                                "default": false
                            }
                        }
                    }
                }
            }
        }
    })
}

fn settings_schema_ui() -> Value {
    json!({
        "sections": [
            {
                "id": "launch",
                "title": "Launch",
                "fields": ["name", "command", "args", "interactive_args", "prompt_mode", "invocation_mode"]
            },
            {
                "id": "state",
                "title": "State",
                "fields": ["resume", "session_capture", "resume_acceptance", "session_storage"]
            },
            {
                "id": "policy",
                "title": "Policy",
                "fields": ["system_prompt_override", "tool_restrictions"]
            },
            {
                "id": "quota",
                "title": "Quota",
                "fields": ["quota_script", "auth_refresh_command"]
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(params: Value) -> String {
        json!({
            "contract": CONTRACT,
            "request_id": "req-test",
            "host": { "app": "test" },
            "params": params,
        })
        .to_string()
    }

    #[test]
    fn schema_requires_known_schema_id() {
        let args = vec!["agent-runner-claude".to_string(), "schema".to_string()];
        let output = handle_invocation(&args, &request(json!({ "schema_id": "missing" })));
        assert_eq!(output.exit_code, 1);
        let body: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["error"]["code"], "unknown_schema");
    }

    #[test]
    fn unsupported_future_capability_uses_contract_error() {
        let args = vec![
            "agent-runner-claude".to_string(),
            "rotation.assess".to_string(),
        ];
        let output = handle_invocation(&args, &request(json!({})));
        assert_eq!(output.exit_code, 3);
        let body: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(body["error"]["category"], "unsupported");
    }

    #[test]
    fn shell_command_timeout_does_not_wait_for_inherited_pipes() {
        let started = std::time::Instant::now();
        let result = run_shell_command("sleep 2", Duration::from_millis(10), CommandKind::Quota);
        assert!(matches!(
            result,
            Err(ShellCommandFailure::Timeout(CommandKind::Quota))
        ));
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "timeout path waited for descendant-held pipes"
        );
    }

    #[test]
    fn auth_timeout_failure_uses_auth_specific_code() {
        let failure = command_failure(
            "req-auth-timeout",
            ShellCommandFailure::Timeout(CommandKind::Auth),
            "quota_refresh_auth_failed",
        );
        assert_eq!(failure.code, "quota_refresh_auth_timeout");
        assert_eq!(failure.category, "timeout");
        assert_eq!(failure.message, "auth_refresh_command timed out");
    }
}
