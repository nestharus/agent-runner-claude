//! Claude plug points for the SDK's shared one-shot launch lifecycle.
//!
//! The SDK owns request custody, exact replay, interrupted-actor discharge and
//! reconciliation, admission against termination signals and the host
//! deadline, the native effect gate, process-group custody, draining,
//! heartbeats and the sealed completion receipt. This module supplies the
//! Claude request digest, the host-supplied argv/environment/stdin, raw byte
//! framing of native output, the `provider_session_known` marker, and Claude
//! terminal classification. Launch keeps this provider's contract: native
//! failures, including commands that cannot be spawned, are reported in the
//! `exit` event and the provider exits 0 once that event is delivered. A
//! command that cannot be spawned is the SDK's observed start failure at the
//! gate's spawn or `exec`, never a prediction from `PATH` or permission bits
//! and never an inference from exit status 126 or stderr text.

use crate::{
    byte_payload_bytes, classify_terminal_signal, now_unix_ms, process_status_from_output,
    sha256_hex, terminal_signal_json, LaunchParams, ProcessStatus, ProviderFailure,
    RequestEnvelope, CONTRACT,
};
use agent_provider_execution::{
    custody::{CustodyError, RequestCustody},
    durable_fs::create_private_directories,
    framing::FramingError,
    lifecycle::{
        self, Channel, EventSink, LaunchAdapter, LaunchSpec, LifecycleError, LifecycleTiming,
        NativeCommand, NativeOutcome, OutputFraming, Preparation, StartFailure, Terminal,
    },
    process::{locate_provider_executable, run_effect_gate, EffectGate, GatedCommand},
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const NATIVE_EFFECT_GATE_ARG: &str = "__native_effect_gate";
const NATIVE_EFFECT_GATE_FD_ENV: &str = "AGENT_RUNNER_CLAUDE_NATIVE_EFFECT_GATE_FD";
const PROVIDER_BINARY: &str = "agent-runner-claude";
const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;

/// Runs the native effect gate inside this provider binary.
pub fn run_native_effect_gate(args: &[String]) -> i32 {
    run_effect_gate(args, NATIVE_EFFECT_GATE_FD_ENV)
}

pub(crate) const LAUNCH_OUTPUT_PROTOCOL: &str = "oulipoly.launch_output/v1";
pub(crate) const LAUNCH_OUTPUT_COMPLETE_MARKER: &str = "oulipoly.launch_output_complete/v1";

/// Whether the launch requested `oulipoly.launch_output/v1` custody. The
/// request is admitted only when its own host environment selected the
/// extension and names exactly that protocol.
fn output_requested(
    request: &RequestEnvelope,
    params: &LaunchParams,
) -> Result<bool, ProviderFailure> {
    let Some(value) = params
        .output_delivery
        .as_ref()
        .filter(|value| !value.is_null())
    else {
        return Ok(false);
    };
    let selected = request
        .host
        .env
        .as_ref()
        .and_then(|env| env.get(crate::HOST_LAUNCH_OUTPUT_ENV))
        .map(String::as_str)
        == Some("1");
    if !selected {
        return Err(lifecycle_failure(
            &request.request_id,
            "launch_output_not_selected",
            "unsupported",
            "params.output_delivery requires host.env.OULIPOLY_HOST_LAUNCH_OUTPUT_V1=1",
            false,
            3,
        ));
    }
    let object = value
        .as_object()
        .filter(|object| object.len() == 1 && object.get("protocol").is_some_and(Value::is_string))
        .ok_or_else(|| {
            lifecycle_failure(
                &request.request_id,
                "invalid_launch_output_request",
                "invalid_request",
                "output_delivery must contain only its protocol",
                false,
                2,
            )
        })?;
    if object["protocol"] != LAUNCH_OUTPUT_PROTOCOL {
        return Err(lifecycle_failure(
            &request.request_id,
            "unsupported_launch_output_protocol",
            "unsupported",
            "Unsupported launch output delivery protocol",
            false,
            3,
        ));
    }
    Ok(true)
}

/// The completion marker: byte counts and SHA-256 of every data event, and
/// their count. It is the last event before `exit` on every exit path.
pub(crate) fn output_complete_marker(accounting: Value) -> Value {
    let mut value = accounting;
    value["protocol"] = json!(LAUNCH_OUTPUT_PROTOCOL);
    json!({"kind":"marker","name":LAUNCH_OUTPUT_COMPLETE_MARKER,"value":value})
}

pub(crate) fn run<W: Write>(
    request: &RequestEnvelope,
    params: LaunchParams,
    writer: &mut W,
) -> Result<i32, ProviderFailure> {
    let output_requested = output_requested(request, &params)?;
    let state_root = state_root(request)?.join("provider-state/claude/launch");
    create_private_directories(&state_root)
        .map_err(|error| failure(&request.request_id, "launch_io", error.to_string()))?;
    let spec = LaunchSpec {
        contract: CONTRACT,
        request_id: &request.request_id,
        provider_instance_id: request.provider_instance_id.as_deref(),
        deadline_unix_ms: request.host.deadline_unix_ms,
        state_root: &state_root,
        timing: LifecycleTiming::default(),
    };
    let mut adapter = ClaudeLaunch {
        request,
        params,
        output_requested,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    lifecycle::run_launch(&spec, &mut adapter, writer).map_err(|mut error: ProviderFailure| {
        error.request_id = request.request_id.clone();
        error
    })
}

pub(crate) fn state_root(request: &RequestEnvelope) -> Result<PathBuf, ProviderFailure> {
    if let Some(root) = &request.host.data_root {
        return Ok(PathBuf::from(root));
    }
    request
        .host
        .env
        .as_ref()
        .and_then(|env| env.get("HOME").cloned())
        .or_else(|| std::env::var("HOME").ok())
        .filter(|home| !home.is_empty())
        .map(|home| Path::new(&home).join(".local/share/oulipoly-agent-runner"))
        .ok_or_else(|| {
            failure(
                &request.request_id,
                "launch_state_unavailable",
                "launch custody requires host.data_root or HOME",
            )
        })
}

struct ClaudeLaunch<'a> {
    request: &'a RequestEnvelope,
    params: LaunchParams,
    output_requested: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ClaudeLaunch<'_> {
    fn session_known(&self) -> bool {
        self.params
            .session
            .as_ref()
            .and_then(|value| value.get("provider_session_id"))
            .and_then(Value::as_str)
            .is_some()
    }

    fn spawn_error(&self, reason: String, session: Option<Value>) -> Terminal {
        let status = ProcessStatus::SpawnError { reason };
        let signal = classify_terminal_signal(&[], &[], &status, now_unix_ms());
        Terminal {
            status: serde_json::to_value(&status).expect("process status serializes"),
            terminal_signal: terminal_signal_json(&signal),
            session,
            exit_code: 0,
        }
    }

    fn session_marker() -> Value {
        json!({"kind":"marker","name":"provider_session_known","value":true})
    }
}

impl LaunchAdapter for ClaudeLaunch<'_> {
    type Failure = ProviderFailure;

    /// Every launch input that determines the native effect. Executable and
    /// provider build identity are deliberately absent.
    fn request_digest(&mut self) -> Result<String, ProviderFailure> {
        let request = self.request;
        Ok(sha256_hex(
            &serde_json::to_vec(&json!({"params":request.params,
                "host_env":request.host.env,
                "host_working_directory":request.host.working_directory}))
            .expect("launch digest input serializes"),
        ))
    }

    fn prepare(&mut self, _custody: &RequestCustody) -> Result<Preparation, ProviderFailure> {
        let params = &self.params;
        // Settled outcomes carry no data events; a requested completion
        // marker reports that empty output.
        let settled_events = || {
            if self.output_requested {
                vec![output_complete_marker(
                    agent_provider_execution::lifecycle::DataAccounting::default().to_json(),
                )]
            } else {
                Vec::new()
            }
        };
        let Some((program, args)) = params.argv.split_first() else {
            return Ok(Preparation::Settled {
                events: settled_events(),
                terminal: self.spawn_error("Empty command".into(), None),
            });
        };
        let stdin = match params.stdin.as_ref().map(byte_payload_bytes).transpose() {
            Ok(stdin) => stdin,
            Err(reason) => {
                return Ok(Preparation::Settled {
                    events: settled_events(),
                    terminal: self.spawn_error(reason, None),
                })
            }
        };
        let executable = locate_provider_executable(PROVIDER_BINARY)
            .map_err(|error| failure(&self.request.request_id, "launch_io", error.to_string()))?;
        let mut command = GatedCommand::new(
            &EffectGate {
                executable: &executable,
                argument: NATIVE_EFFECT_GATE_ARG,
                descriptor_env: NATIVE_EFFECT_GATE_FD_ENV,
            },
            program,
            args,
        )
        .map_err(|error| failure(&self.request.request_id, "launch_io", error.to_string()))?;
        command
            .command_mut()
            .current_dir(&params.working_directory)
            .envs(&params.env);
        Ok(Preparation::Native(NativeCommand {
            command,
            stdin,
            framing: OutputFraming::Chunks {
                max_bytes: OUTPUT_CHUNK_BYTES,
            },
        }))
    }

    /// The command could not be spawned: its working directory or the gate
    /// process could not be set up, or `exec` of the program failed. Reported
    /// as the contract's `spawn_error` with the actual OS error, as when the
    /// provider spawned the command directly.
    fn start_failed<W: Write>(
        &mut self,
        failure: &StartFailure,
        events: &mut EventSink<'_, W>,
    ) -> Result<Option<Terminal>, ProviderFailure> {
        let (StartFailure::Spawn(error) | StartFailure::Exec(error)) = failure;
        if self.session_known() {
            events.event(Self::session_marker())?;
        }
        if self.output_requested {
            events.event(output_complete_marker(events.accounting().to_json()))?;
        }
        Ok(Some(self.spawn_error(
            format!("Failed to spawn Claude provider command: {error}"),
            self.params.session.clone(),
        )))
    }

    fn started<W: Write>(&mut self, events: &mut EventSink<'_, W>) -> Result<(), ProviderFailure> {
        if self.session_known() {
            events.event(Self::session_marker())?;
        }
        Ok(())
    }

    fn output<W: Write>(
        &mut self,
        channel: Channel,
        bytes: Vec<u8>,
        events: &mut EventSink<'_, W>,
    ) -> Result<(), ProviderFailure> {
        events.data(channel, &bytes)?;
        match channel {
            Channel::Stdout => self.stdout.extend(bytes),
            Channel::Stderr => self.stderr.extend(bytes),
        }
        Ok(())
    }

    fn finish<W: Write>(
        &mut self,
        outcome: NativeOutcome,
        events: &mut EventSink<'_, W>,
    ) -> Result<Terminal, ProviderFailure> {
        if self.output_requested {
            events.event(output_complete_marker(events.accounting().to_json()))?;
        }
        let status = match outcome.stopped {
            Some(_) => ProcessStatus::Cancelled,
            None => process_status_from_output(&outcome.status),
        };
        let signal = classify_terminal_signal(&self.stdout, &self.stderr, &status, now_unix_ms());
        Ok(Terminal {
            status: serde_json::to_value(&status).expect("process status serializes"),
            terminal_signal: terminal_signal_json(&signal),
            session: self.params.session.clone(),
            exit_code: 0,
        })
    }
}

pub(crate) fn failure(
    request_id: &str,
    code: &'static str,
    message: impl Into<String>,
) -> ProviderFailure {
    lifecycle_failure(request_id, code, "failed", message, false, 1)
}

fn lifecycle_failure(
    request_id: &str,
    code: &'static str,
    category: &'static str,
    message: impl Into<String>,
    retryable: bool,
    exit_code: i32,
) -> ProviderFailure {
    ProviderFailure {
        request_id: request_id.to_string(),
        code,
        category,
        message: message.into(),
        retryable,
        details: json!({}),
        exit_code,
    }
}

fn custody_failure(error: CustodyError) -> ProviderFailure {
    match error {
        CustodyError::Busy => lifecycle_failure(
            "",
            "launch_busy",
            "conflict",
            "This request is already executing",
            true,
            2,
        ),
        CustodyError::InvalidState => failure("", "launch_state_invalid", "Invalid launch state"),
        CustodyError::StateWrite(message) => failure("", "launch_state_write", message),
        CustodyError::JournalMissing | CustodyError::JournalMismatch => {
            failure("", "launch_journal_invalid", error.to_string())
        }
        CustodyError::JournalOverflow => failure("", "launch_output_accounting", error.to_string()),
        CustodyError::Io(error) => failure("", "launch_io", error.to_string()),
    }
}

/// Claude failure codes and categories for the shared lifecycle's outcomes.
impl From<LifecycleError> for ProviderFailure {
    fn from(error: LifecycleError) -> Self {
        let message = error.to_string();
        match error {
            LifecycleError::Busy => custody_failure(CustodyError::Busy),
            LifecycleError::RequestChanged => {
                lifecycle_failure("", "request_changed", "conflict", message, false, 2)
            }
            LifecycleError::ReconciliationRequired => lifecycle_failure(
                "",
                "launch_reconciliation_required",
                "conflict",
                "Prior invocation ended before terminal custody; inspect the Claude session before issuing a new request",
                false,
                2,
            ),
            LifecycleError::Cancelled => failure("", "launch_cancelled", message),
            LifecycleError::DeadlineElapsed => {
                lifecycle_failure("", "launch_deadline", "timeout", message, false, 1)
            }
            LifecycleError::Custody(error) => custody_failure(error),
            LifecycleError::Framing(FramingError::Overflow) => {
                failure("", "launch_output_accounting", message)
            }
            LifecycleError::Framing(FramingError::Io(_)) | LifecycleError::Io(_) => {
                failure("", "launch_io", message)
            }
            LifecycleError::NativeStreamInvalid => failure("", "native_stream_invalid", message),
            LifecycleError::NativeStreamsClosed => failure("", "native_streams_closed", message),
            LifecycleError::NativeDrainIncomplete => {
                failure("", "native_stream_drain_incomplete", message)
            }
            LifecycleError::InputStalled
            | LifecycleError::InputWriterFailed
            | LifecycleError::InputIncomplete => failure("", "stdin_failed", message),
            LifecycleError::WaitFailed => failure("", "native_wait_failed", message),
            LifecycleError::AccountingOverflow => failure("", "launch_output_accounting", message),
        }
    }
}

/// A native command behind this provider's effect gate.
pub(crate) fn gated_command(
    request_id: &str,
    program: &str,
    args: &[String],
) -> Result<GatedCommand, ProviderFailure> {
    let executable = locate_provider_executable(PROVIDER_BINARY)
        .map_err(|error| failure(request_id, "launch_io", error.to_string()))?;
    GatedCommand::new(
        &EffectGate {
            executable: &executable,
            argument: NATIVE_EFFECT_GATE_ARG,
            descriptor_env: NATIVE_EFFECT_GATE_FD_ENV,
        },
        program,
        args,
    )
    .map_err(|error| failure(request_id, "launch_io", error.to_string()))
}
