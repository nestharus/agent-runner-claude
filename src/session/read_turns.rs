// declared_role: accessor, filter, formatter, mapper, orchestration

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::native_claude::{parse_native_jsonl, turns_for_session, NativeTurn};
use super::storage::{locate_by_session_id, read_transcript, scan_bounds, LocateOutcome};
use super::types::{conflict, failed, host_home, required_session_id};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let session_id = required_session_id(request)?;
    let path = transcript_path(request, &session_id)?;
    let text = transcript_text(&path)?;
    let parsed = parse_native_jsonl(&text);
    let turns = turn_values(turns_after(
        turns_for_session(&parsed, &session_id),
        after_turn_id(&request.params),
    ));
    Ok(read_turns_response(turns, parsed.complete))
}

fn transcript_path(
    request: &RequestEnvelope,
    session_id: &str,
) -> Result<std::path::PathBuf, ProviderFailure> {
    if let Some(path) = requested_path(&request.params) {
        return confined_requested_path(request, path);
    }
    let home = host_home(request)?;
    match locate_by_session_id(&home, session_id, scan_bounds(&request.params)).map_err(
        |error| {
            failed(
                "session_locate_failed",
                format!("failed to scan Claude transcripts: {error}"),
            )
        },
    )? {
        LocateOutcome::Found(path) => Ok(path),
        LocateOutcome::Missing => Err(super::types::invalid_request(
            "session_transcript_not_found",
            "session transcript was not found",
        )),
        LocateOutcome::Ambiguous(_) => Err(conflict(
            "ambiguous_session_transcript",
            "session_id matched multiple Claude transcripts",
        )),
    }
}

fn transcript_text(path: &std::path::Path) -> Result<String, ProviderFailure> {
    read_transcript(path).map_err(|error| {
        failed(
            "session_read_failed",
            format!("failed to read Claude transcript: {error}"),
        )
    })
}

fn requested_path(params: &Value) -> Option<&str> {
    params
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn confined_requested_path(
    request: &RequestEnvelope,
    path: &str,
) -> Result<std::path::PathBuf, ProviderFailure> {
    let home = host_home(request)?;
    super::storage::confined_transcript_path(&home, path).map_err(|error| {
        conflict(
            "transcript_path_outside_provider_root",
            format!("session transcript path is not provider-owned: {error}"),
        )
    })
}

fn after_turn_id(params: &Value) -> Option<String> {
    params
        .get("after_turn_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn turns_after(turns: Vec<NativeTurn>, after_turn_id: Option<String>) -> Vec<NativeTurn> {
    let Some(after_turn_id) = after_turn_id else {
        return turns;
    };
    let Some(index) = turn_index(&turns, &after_turn_id) else {
        return turns;
    };
    turns.into_iter().skip(index + 1).collect()
}

fn turn_index(turns: &[NativeTurn], turn_id: &str) -> Option<usize> {
    turns.iter().position(|turn| turn.id == turn_id)
}

fn turn_values(turns: Vec<NativeTurn>) -> Vec<Value> {
    turns.into_iter().map(turn_value).collect()
}

fn turn_value(turn: NativeTurn) -> Value {
    json!({
        "id": turn.id,
        "role": turn.role,
        "body": turn.body,
    })
}

fn read_turns_response(turns: Vec<Value>, complete: bool) -> Value {
    json!({
        "turn_count": turns.len(),
        "turns": turns,
        "complete": complete,
    })
}
