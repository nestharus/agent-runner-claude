// declared_role: accessor, formatter, mapper, orchestration, parser, validator
// adapter_declarations:
//   - component: src/session/replace.rs
//     role: adapter
//     Translates:
//       - contract/v1/session.schema.json#/$defs/SessionReplaceRequest
//       - contract/v1/session.schema.json#/$defs/SessionReplaceResult
//       - src/session/storage.rs confined transcript path seam
//       - src/session/native_claude.rs native transcript render seam
//       - src/encoding.rs canonical transcript base64/hash seam

use std::fs;

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::canonical::{canonical_format_id, parse_canonical, turn_count};
use super::native_claude::render_native_jsonl;
use super::types::{conflict, failed, host_home, required_session_id, required_string};

struct ReplacementInput {
    session_id: String,
    path: std::path::PathBuf,
    canonical_bytes: Vec<u8>,
    records: Vec<super::canonical::CanonicalRecord>,
}

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let input = replacement_input(request)?;
    let current = read_current(&input.path)?;
    require_preimage_match(&current, preimage_sha256(&request.params))?;
    let native_bytes = render_native_jsonl(&input.session_id, &input.records);
    if native_bytes == current {
        return Ok(unchanged_response());
    }

    write_replacement(&input.path, &native_bytes)?;
    let actual = read_replaced(&input.path)?;
    Ok(changed_response(&input, &actual))
}

fn replacement_input(request: &RequestEnvelope) -> Result<ReplacementInput, ProviderFailure> {
    let session_id = required_session_id(request)?;
    let path = confined_path(request)?;
    require_canonical_format(&request.params)?;
    let canonical_bytes = canonical_bytes(&request.params)?;
    let records = parse_canonical(&canonical_bytes)?;
    Ok(ReplacementInput {
        session_id,
        path,
        canonical_bytes,
        records,
    })
}

fn confined_path(request: &RequestEnvelope) -> Result<std::path::PathBuf, ProviderFailure> {
    let raw_path = required_string(&request.params, "path")?;
    let home = host_home(request)?;
    super::storage::confined_transcript_path(&home, &raw_path).map_err(|error| {
        conflict(
            "transcript_path_outside_provider_root",
            format!("session.replace transcript path is not provider-owned: {error}"),
        )
    })
}

fn require_canonical_format(params: &Value) -> Result<(), ProviderFailure> {
    let canonical_format = required_string(params, "canonical_format")?;
    if canonical_format == canonical_format_id() {
        Ok(())
    } else {
        Err(super::types::invalid_request(
            "unsupported_canonical_format",
            format!("unsupported canonical transcript format: {canonical_format}"),
        ))
    }
}

fn read_current(path: &std::path::Path) -> Result<Vec<u8>, ProviderFailure> {
    fs::read(path).map_err(|error| {
        failed(
            "session_replace_read_failed",
            format!("failed to read current transcript: {error}"),
        )
    })
}

fn preimage_sha256(params: &Value) -> Option<String> {
    params
        .get("preimage_sha256")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn require_preimage_match(
    current: &[u8],
    preimage_sha256: Option<String>,
) -> Result<(), ProviderFailure> {
    let Some(preimage_sha256) = preimage_sha256 else {
        return Ok(());
    };
    if crate::encoding::sha256_hex(current) == preimage_sha256 {
        Ok(())
    } else {
        Err(conflict(
            "preimage_sha256_mismatch",
            "current transcript sha256 does not match supplied preimage_sha256",
        ))
    }
}

fn write_replacement(path: &std::path::Path, bytes: &[u8]) -> Result<(), ProviderFailure> {
    super::atomic::write_transcript_atomic(path, bytes).map_err(|error| {
        failed(
            "session_replace_write_failed",
            format!("failed to atomically write transcript: {error}"),
        )
    })
}

fn read_replaced(path: &std::path::Path) -> Result<Vec<u8>, ProviderFailure> {
    fs::read(path).map_err(|error| {
        failed(
            "session_replace_readback_failed",
            format!("failed to read replaced transcript: {error}"),
        )
    })
}

fn canonical_bytes(params: &Value) -> Result<Vec<u8>, ProviderFailure> {
    let payload = canonical_payload(params)?;
    require_canonical_payload_kind(canonical_payload_kind(payload))?;
    let data_base64 = canonical_payload_data(payload)?;
    decode_canonical_payload(data_base64)
}

fn canonical_payload(params: &Value) -> Result<&Value, ProviderFailure> {
    params.get("canonical_transcript").ok_or_else(|| {
        super::types::invalid_request(
            "missing_canonical_transcript",
            "session.replace requires canonical_transcript",
        )
    })
}

fn canonical_payload_kind(payload: &Value) -> &str {
    payload
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("bytes")
}

fn require_canonical_payload_kind(kind: &str) -> Result<(), ProviderFailure> {
    if kind != "bytes" {
        Err(super::types::invalid_request(
            "unsupported_canonical_payload",
            format!("unsupported canonical_transcript kind: {kind}"),
        ))
    } else {
        Ok(())
    }
}

fn canonical_payload_data(payload: &Value) -> Result<&str, ProviderFailure> {
    payload
        .get("data_base64")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            super::types::invalid_request(
                "missing_canonical_data",
                "canonical_transcript requires data_base64",
            )
        })
}

fn decode_canonical_payload(data_base64: &str) -> Result<Vec<u8>, ProviderFailure> {
    crate::encoding::decode_base64(data_base64).map_err(|error| {
        super::types::invalid_request(
            "invalid_canonical_base64",
            format!("canonical transcript data_base64 is invalid: {error}"),
        )
    })
}

fn unchanged_response() -> Value {
    json!({ "changed": false, "artifacts": [] })
}

fn changed_response(input: &ReplacementInput, actual: &[u8]) -> Value {
    let postimage_sha256 = crate::encoding::sha256_hex(actual);
    let artifacts = artifacts(&input.path, &postimage_sha256);
    json!({
        "changed": true,
        "postimage_sha256": postimage_sha256,
        "artifacts": artifacts,
        "host_state_plan": host_state_plan(input, &postimage_sha256, &artifacts),
    })
}

fn artifacts(path: &std::path::Path, postimage_sha256: &str) -> Value {
    json!([{
        "kind": "transcript",
        "path": path.display().to_string(),
        "sha256": postimage_sha256,
    }])
}

fn host_state_plan(input: &ReplacementInput, postimage_sha256: &str, artifacts: &Value) -> Value {
    json!({
        "schema_version": 1,
        "operation": "session.replace",
        "session_id": input.session_id,
        "provider_name": "claude",
        "canonical_format": canonical_format_id(),
        "turn_count": turn_count(&input.records),
        "records_sha256": crate::encoding::sha256_hex(&input.canonical_bytes),
        "postimage_sha256": postimage_sha256,
        "artifacts": artifacts,
    })
}
