// declared_role: mapper, orchestration, parser, accessor

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    Exited(i64),
    SignalTerminated(i64),
    SpawnError,
    ProlongedSilence,
    Cancelled,
    Unknown,
}

pub fn parse_status(value: &Value) -> Option<ProcessStatus> {
    map_status(status_fields(value)?)
}

struct StatusFields<'a> {
    kind: &'a str,
    code: Option<i64>,
    signal: Option<i64>,
    reason: Option<&'a str>,
}

fn status_fields(value: &Value) -> Option<StatusFields<'_>> {
    let object = value.as_object()?;
    Some(StatusFields {
        kind: object.get("kind")?.as_str()?,
        code: object.get("code").and_then(Value::as_i64),
        signal: object.get("signal").and_then(Value::as_i64),
        reason: object.get("reason").and_then(Value::as_str),
    })
}

fn map_status(fields: StatusFields<'_>) -> Option<ProcessStatus> {
    match fields.kind {
        "exited" => Some(ProcessStatus::Exited(fields.code?)),
        "signal_terminated" => Some(ProcessStatus::SignalTerminated(fields.signal?)),
        "spawn_error" => fields.reason.map(|_| ProcessStatus::SpawnError),
        "prolonged_silence" => fields.reason.map(|_| ProcessStatus::ProlongedSilence),
        "cancelled" => Some(ProcessStatus::Cancelled),
        "unknown" => Some(ProcessStatus::Unknown),
        _ => None,
    }
}
