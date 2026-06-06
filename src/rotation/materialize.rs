// declared_role: accessor, formatter, mapper, orchestration, parser, validator
// adapter_declarations:
//   - component: src/rotation/materialize.rs
//     role: adapter
//     Translates:
//       - contract/v1/rotation.schema.json#/$defs/RotationMaterializeRequest
//       - contract/v1/rotation.schema.json#/$defs/RotationMaterializeResult
//       - contract/v1/rotation.schema.json#/$defs/RotationHostStatePlan
//       - src/fs/paths.rs artifact-root confinement and safe filename seam
//       - src/encoding.rs canonical transcript base64 seam

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::{ErrorCategory, ProviderFailure};

struct MaterializationInput {
    bytes: Vec<u8>,
    target_session_id: String,
    chain_id: String,
    root: PathBuf,
}

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let input = materialization_input(request)?;
    let artifact_path = artifact_path(&input)?;
    let artifact = super::artifacts::write_file_artifact(&artifact_path, &input.bytes)?;
    let artifacts = vec![artifact];
    Ok(materialization_response(request, &input, &artifacts))
}

fn materialization_input(
    request: &RequestEnvelope,
) -> Result<MaterializationInput, ProviderFailure> {
    let params = request.params.as_object().ok_or_else(invalid_params)?;
    Ok(MaterializationInput {
        bytes: canonical_transcript_bytes(&request.params)?,
        target_session_id: filename_param(params, "target_session_id", "target-session")?,
        chain_id: filename_param(params, "chain_id", "rotation-chain")?,
        root: artifact_root(&request.host, &request.params)?,
    })
}

fn filename_param(
    params: &serde_json::Map<String, Value>,
    key: &str,
    default: &str,
) -> Result<String, ProviderFailure> {
    let value = filename_param_value(params, key, default);
    require_safe_filename_segment(value, key)?;
    Ok(value.to_string())
}

fn filename_param_value<'a>(
    params: &'a serde_json::Map<String, Value>,
    key: &str,
    default: &'a str,
) -> &'a str {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

fn require_safe_filename_segment(value: &str, key: &str) -> Result<(), ProviderFailure> {
    if crate::fs::paths::safe_filename_segment(value) {
        Ok(())
    } else {
        Err(invalid_rotation_filename_segment(key))
    }
}

fn invalid_rotation_filename_segment(key: &str) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_rotation_filename_segment",
        format!("rotation.materialize {key} must be a single filename segment"),
    )
}

fn artifact_path(input: &MaterializationInput) -> Result<PathBuf, ProviderFailure> {
    let candidate = artifact_path_candidate(input);
    confined_artifact_path(&input.root, &candidate)
}

fn artifact_path_candidate(input: &MaterializationInput) -> PathBuf {
    input.root.join("rotation").join(format!(
        "{}-{}.canonical.jsonl",
        input.chain_id, input.target_session_id
    ))
}

fn confined_artifact_path(root: &Path, candidate: &Path) -> Result<PathBuf, ProviderFailure> {
    crate::fs::paths::confined_child_path(root, candidate).map_err(rotation_artifact_path_conflict)
}

fn rotation_artifact_path_conflict(
    error: crate::fs::paths::PathConfinementError,
) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Conflict,
        "rotation_artifact_path_outside_provider_root",
        format!("rotation artifact path is not provider-owned: {error}"),
        false,
    )
}

fn materialization_response(
    request: &RequestEnvelope,
    input: &MaterializationInput,
    artifacts: &[Value],
) -> Value {
    json!({
        "changed": true,
        "target_provider_session_id": input.target_session_id,
        "artifacts": artifacts,
        "host_state_plan": super::host_plan::materialize_plan(&request.params, artifacts)
    })
}

fn canonical_transcript_bytes(params: &Value) -> Result<Vec<u8>, ProviderFailure> {
    let payload = canonical_transcript_payload(params)?;
    let fields = canonical_transcript_fields(payload)?;
    require_canonical_transcript_kind(fields.kind)?;
    decode_canonical_transcript_data(fields.data_base64)
}

struct CanonicalTranscriptFields<'a> {
    kind: &'a str,
    data_base64: &'a str,
}

fn canonical_transcript_payload(
    params: &Value,
) -> Result<&serde_json::Map<String, Value>, ProviderFailure> {
    params
        .get("canonical_transcript")
        .and_then(Value::as_object)
        .ok_or_else(invalid_params)
}

fn canonical_transcript_fields(
    payload: &serde_json::Map<String, Value>,
) -> Result<CanonicalTranscriptFields<'_>, ProviderFailure> {
    Ok(CanonicalTranscriptFields {
        kind: payload
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(invalid_params)?,
        data_base64: payload
            .get("data_base64")
            .and_then(Value::as_str)
            .ok_or_else(invalid_params)?,
    })
}

fn require_canonical_transcript_kind(kind: &str) -> Result<(), ProviderFailure> {
    if kind == "bytes" {
        Ok(())
    } else {
        Err(invalid_params())
    }
}

fn decode_canonical_transcript_data(data_base64: &str) -> Result<Vec<u8>, ProviderFailure> {
    crate::encoding::decode_base64(data_base64).map_err(invalid_rotation_canonical_transcript)
}

fn invalid_rotation_canonical_transcript(error: String) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_rotation_canonical_transcript",
        format!("canonical transcript payload is invalid base64: {error}"),
    )
}

fn artifact_root(host: &Value, params: &Value) -> Result<PathBuf, ProviderFailure> {
    let data_root = host_data_root(host)?;
    let provider_roots = provider_artifact_roots(&data_root);
    let requested_root = normalized_requested_artifact_root(&data_root, params);
    confined_artifact_root(&requested_root, &provider_roots)
}

fn normalized_requested_artifact_root(data_root: &Path, params: &Value) -> PathBuf {
    let requested_root =
        requested_artifact_root(params).unwrap_or_else(|| default_artifact_root(data_root));
    crate::fs::paths::normalized_absolute(&requested_root, data_root)
}

fn default_artifact_root(data_root: &Path) -> PathBuf {
    data_root.join("claude").join("rotation-artifacts")
}

fn confined_artifact_root(
    requested_root: &Path,
    provider_roots: &[PathBuf],
) -> Result<PathBuf, ProviderFailure> {
    provider_roots
        .iter()
        .find_map(|provider_root| {
            crate::fs::paths::confined_path_or_root(provider_root, requested_root).ok()
        })
        .ok_or_else(|| outside_provider_root(requested_root, provider_roots))
}

fn provider_artifact_roots(data_root: &Path) -> Vec<PathBuf> {
    vec![crate::fs::paths::normalized_absolute(
        &data_root.join("claude"),
        data_root,
    )]
}

fn host_data_root(host: &Value) -> Result<PathBuf, ProviderFailure> {
    host_data_root_value(host)
        .filter(non_empty_string)
        .map(PathBuf::from)
        .ok_or_else(missing_host_data_root)
}

fn host_data_root_value(host: &Value) -> Option<&str> {
    host.get("data_root").and_then(Value::as_str)
}

fn non_empty_string(value: &&str) -> bool {
    !value.is_empty()
}

fn missing_host_data_root() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "missing_host_data_root",
        "host.data_root is required for rotation artifact path resolution",
    )
}

fn requested_artifact_root(params: &Value) -> Option<PathBuf> {
    params
        .get("provider_artifact_root")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn outside_provider_root(path: &Path, provider_roots: &[PathBuf]) -> ProviderFailure {
    ProviderFailure::new(
        ErrorCategory::Conflict,
        "rotation_artifact_root_outside_provider_root",
        format!(
            "rotation artifact root {} is outside provider-owned roots {}",
            path.display(),
            provider_roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        false,
    )
}

fn invalid_params() -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_rotation_materialize_params",
        "rotation.materialize params do not match the rotation contract",
    )
}
