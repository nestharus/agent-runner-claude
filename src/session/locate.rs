// declared_role: formatter, orchestration

use serde_json::json;
use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::native_claude::native_format_id;
use super::storage::{locate_by_session_id, scan_bounds, LocateOutcome};
use super::types::{conflict, host_home, required_session_id};

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let session_id = required_session_id(request)?;
    let home = host_home(request)?;
    let bounds = scan_bounds(&request.params);
    match locate_by_session_id(&home, &session_id, bounds).map_err(session_locate_failed)? {
        LocateOutcome::Missing => Ok(missing_response()),
        LocateOutcome::Found(path) => Ok(found_response(&path, &session_id)),
        LocateOutcome::Ambiguous(paths) => Err(conflict(
            "ambiguous_session_transcript",
            ambiguous_message(&paths),
        )),
    }
}

fn session_locate_failed(error: std::io::Error) -> ProviderFailure {
    super::types::failed(
        "session_locate_failed",
        format!("failed to scan Claude transcripts: {error}"),
    )
}

fn missing_response() -> Value {
    json!({ "located": false })
}

fn found_response(path: &std::path::Path, session_id: &str) -> Value {
    json!({
        "located": true,
        "path": path.display().to_string(),
        "format_id": native_format_id(),
        "source_id": session_id,
        "require_existing_observed": true,
    })
}

fn ambiguous_message(paths: &[std::path::PathBuf]) -> String {
    format!(
        "session_id matched multiple Claude transcripts: {}",
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}
