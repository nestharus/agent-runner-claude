// declared_role: orchestration, validator, mapper, predicate, formatter, accessor

pub mod child;
pub mod drain;
pub mod events;
pub mod params;
pub mod session_marker;
pub mod stdin;

use serde_json::json;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

pub fn handle(_subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    launch(request)
}

pub fn stream_rejected_request_and_exit(request_id: &str, reason: &str) -> ! {
    stream_pre_spawn_exit(request_id, reason)
}

fn launch(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = params::params_value(&request.params);
    if let Err(message) = validate_required_params(params) {
        stream_pre_spawn_exit(&request.request_id, &message);
    }

    let argv = match params::argv(params) {
        Ok(argv) => argv,
        Err(message) => stream_pre_spawn_exit(&request.request_id, &message),
    };
    let cwd = match params::required_string(params, "working_directory") {
        Ok(cwd) => cwd,
        Err(message) => stream_pre_spawn_exit(&request.request_id, &message),
    };
    let stdin_bytes = match launch_stdin_bytes(params) {
        Ok(bytes) => bytes,
        Err(failure) => stream_pre_spawn_exit(&request.request_id, &failure.message),
    };
    let env = match launch_env(request, params) {
        Ok(env) => env,
        Err(failure) => stream_pre_spawn_exit(&request.request_id, &failure.message),
    };
    let deadline = deadline_unix_ms(request);

    stream_launch_and_exit(&request.request_id, &argv, cwd, &env, stdin_bytes, deadline);
}

fn stream_pre_spawn_exit(request_id: &str, reason: &str) -> ! {
    let stdout = io::stdout();
    let mut events = events::EventWriter::new(stdout.lock(), request_id);
    let status = spawn_error_status(&io::Error::new(io::ErrorKind::InvalidInput, reason));
    let signal = terminal_signal(&status);
    let _ = events.exit(status, signal);
    std::process::exit(0);
}

fn validate_required_params(params: &Value) -> Result<(), String> {
    let object = params
        .as_object()
        .ok_or_else(|| "launch params must be an object".to_string())?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "settings_id"
                | "mode"
                | "model"
                | "argv"
                | "working_directory"
                | "env"
                | "stdin"
                | "session"
        ) {
            return Err(format!("unsupported launch param field: {key}"));
        }
    }
    params::required_string(params, "settings_id")?;
    params::required_string(params, "mode")?;
    params::required_string(params, "working_directory")?;
    if !params.get("model").is_some_and(Value::is_object) {
        return Err("launch params missing model".to_string());
    }
    let _ = params::argv(params)?;
    Ok(())
}

fn launch_env(
    request: &RequestEnvelope,
    params: &Value,
) -> Result<BTreeMap<String, String>, ProviderFailure> {
    let mut env = BTreeMap::new();
    if let Some(host_env) = request.host.get("env") {
        let entries = validate_env_object(host_env)?;
        merge_env(&mut env, &entries);
    }
    if let Some(param_env) = params.get("env") {
        let entries = validate_env_object(param_env)?;
        merge_env(&mut env, &entries);
    }
    Ok(env)
}

fn launch_stdin_bytes(params: &Value) -> Result<Vec<u8>, ProviderFailure> {
    let Some(payload) = params.get("stdin") else {
        return Ok(Vec::new());
    };

    stdin::decode_stdin_payload(payload).map_err(invalid_launch_stdin)
}

fn deadline_unix_ms(request: &RequestEnvelope) -> Option<u64> {
    request.host.get("deadline_unix_ms").and_then(Value::as_u64)
}

fn validate_env_object(value: &Value) -> Result<Vec<(&String, &str)>, ProviderFailure> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_launch_params("launch env must be an object"))?;
    object
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key, value))
                .ok_or_else(|| invalid_launch_params("launch env values must be strings"))
        })
        .collect()
}

fn merge_env(env: &mut BTreeMap<String, String>, entries: &[(&String, &str)]) {
    for &(key, value) in entries {
        env.insert(key.clone(), value.to_string());
    }
}

fn stream_launch_and_exit(
    request_id: &str,
    argv: &[String],
    cwd: &str,
    env: &BTreeMap<String, String>,
    stdin_bytes: Vec<u8>,
    deadline: Option<u64>,
) -> ! {
    let stdout = io::stdout();
    let mut events = events::EventWriter::new(stdout.lock(), request_id);
    let status = match child::RunningChild::spawn(argv, cwd, env, stdin_bytes) {
        Ok((mut child, pipes)) => {
            let _ = events.marker(session_marker::initial_marker_name(), json!(true));
            let _ = events.heartbeat("child_spawned");
            let (sender, receiver) = mpsc::channel();
            let stdout_thread = drain::spawn_drain("stdout", pipes.stdout, sender.clone());
            let stderr_thread = drain::spawn_drain("stderr", pipes.stderr, sender);

            loop {
                match receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(event) => {
                        let _ = events.data(event.channel, &event.bytes);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }

                if deadline_elapsed(deadline) {
                    break;
                }
            }

            let status = child.wait_with_deadline(deadline);
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            while let Ok(event) = receiver.try_recv() {
                let _ = events.data(event.channel, &event.bytes);
            }
            status
        }
        Err(error) => spawn_error_status(&error),
    };
    let signal = terminal_signal(&status);
    let _ = events.exit(status, signal);
    std::process::exit(0);
}

fn deadline_elapsed(deadline: Option<u64>) -> bool {
    deadline.is_some_and(|deadline| crate::encoding::now_unix_ms() >= deadline)
}

fn terminal_signal(status: &Value) -> Value {
    let now = crate::encoding::now_unix_ms();
    let kind = match status.get("kind").and_then(Value::as_str) {
        Some("exited") if status.get("code").and_then(Value::as_i64) == Some(0) => "clean_exit",
        Some("exited") => "nonzero_exit",
        Some("signal_terminated") => "signal_exit",
        Some("spawn_error") => "spawn_error",
        Some("cancelled") => "cancelled",
        Some("prolonged_silence") => "prolonged_silence",
        _ => "unknown",
    };
    json!({ "kind": kind, "observed_at_unix_ms": now })
}

fn spawn_error_status(error: &io::Error) -> Value {
    json!({ "kind": "spawn_error", "reason": error.to_string() })
}

fn invalid_launch_stdin(error: String) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_launch_stdin",
        format!("invalid launch stdin: {error}"),
    )
}

fn invalid_launch_params(message: impl Into<String>) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::InvalidRequest,
        "invalid_launch_params",
        message.into(),
        false,
    )
}
