// declared_role: accessor, formatter, mapper, orchestration, parser, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::canonical::{canonical_format_id, serialize_canonical, turn_count};
use super::native_claude::{canonical_records_for_session, parse_native_jsonl, NativeParse};
use super::storage::{locate_by_session_id, read_transcript, scan_bounds, LocateOutcome};
use super::types::{conflict, failed, host_home, required_session_id};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let session_id = required_session_id(request)?;
    let path = transcript_path(request, &session_id)?;
    let parsed = complete_native_parse(&transcript_text(&path)?)?;
    let records = canonical_records_for_session(&parsed, &session_id);
    let bytes = serialize_canonical(&records);
    Ok(export_response(&bytes, turn_count(&records)))
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
            "session_export_read_failed",
            format!("failed to read Claude transcript: {error}"),
        )
    })
}

fn complete_native_parse(text: &str) -> Result<NativeParse, ProviderFailure> {
    require_complete_parse(parse_native_jsonl(text))
}

fn require_complete_parse(parsed: NativeParse) -> Result<NativeParse, ProviderFailure> {
    if parsed.complete {
        Ok(parsed)
    } else {
        Err(super::types::invalid_request(
            "partial_native_transcript",
            "native transcript contains malformed JSONL",
        ))
    }
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

fn export_response(bytes: &[u8], turn_count: usize) -> Value {
    json!({
        "canonical_format": canonical_format_id(),
        "data_base64": crate::encoding::encode_base64(bytes),
        "turn_count": turn_count,
        "sha256": crate::encoding::sha256_hex(bytes),
    })
}
