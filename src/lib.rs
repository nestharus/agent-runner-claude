use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
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
    "quota.source",
    "quota.probe",
    "quota.refresh_auth",
    "session.locate_transcript",
    "session.read_turns",
    "session.capture",
    "session.export",
    "session.replace",
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
            "quota": false,
            "session": false,
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
            "quota.source".to_string(),
        ];
        let output = handle_invocation(&args, &request(json!({})));
        assert_eq!(output.exit_code, 3);
        let body: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(body["error"]["category"], "unsupported");
    }
}
