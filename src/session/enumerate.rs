// declared_role: accessor, filter, formatter, mapper, orchestration, parser, validator
// adapter_declarations:
//   - component: src/session/enumerate.rs
//     role: adapter
//     Translates:
//       - contract/v1/session.schema.json#/$defs/SessionEnumerateRequest
//       - contract/v1/session.schema.json#/$defs/SessionEnumerateResult
//       - src/session/storage.rs bounded Claude projects transcript scan

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

use super::storage::{
    claude_projects_dir, require_jsonl_transcript, scan_bounds, scan_transcript_files, ScanBounds,
    TranscriptPathError, TranscriptScan,
};
use super::types::{failed, host_home, invalid_request, required_string};

#[derive(Debug, Clone)]
struct EnumerateParams {
    limit: Option<usize>,
    include_cwd: bool,
    include_turn_count: bool,
    since_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct SessionEnumerateResult {
    sessions: Vec<SessionEntry>,
    complete: bool,
    next_cursor: Option<String>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SessionEntry {
    provider_session_id: String,
    title: Option<String>,
    cwd: Option<String>,
    created_unix_ms: Option<i64>,
    updated_unix_ms: Option<i64>,
    turn_count: Option<usize>,
    source: SessionSource,
}

#[derive(Debug, Clone, Serialize)]
struct SessionSource {
    kind: &'static str,
    detail: Option<String>,
}

#[derive(Debug, Clone)]
struct MetadataRecords {
    records: Vec<Value>,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct BoundedTranscriptText {
    text: String,
    truncated: bool,
}

pub fn enumerate_params(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let params = parse_enumerate_params(&request.params)?;
    let home = host_home(request)?;
    let bounds = scan_bounds(&request.params);
    let result = enumerate_home(&home, &params, bounds)?;
    serde_json::to_value(result).map_err(format_response_failed)
}

fn parse_enumerate_params(params: &Value) -> Result<EnumerateParams, ProviderFailure> {
    let object = params.as_object().ok_or_else(invalid_enumerate_params)?;
    let _ = required_string(params, "settings_id")?;
    Ok(EnumerateParams {
        limit: optional_limit(object)?,
        include_cwd: optional_bool(object, "include_cwd")?,
        include_turn_count: optional_bool(object, "include_turn_count")?,
        since_unix_ms: optional_since_unix_ms(object)?,
    })
}

fn invalid_enumerate_params() -> ProviderFailure {
    invalid_request(
        "invalid_session_enumerate_params",
        "session.enumerate params must be an object",
    )
}

fn optional_limit(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<usize>, ProviderFailure> {
    let Some(value) = object.get("limit") else {
        return Ok(None);
    };
    let Some(limit) = value.as_u64().filter(|limit| *limit >= 1) else {
        return Err(invalid_request(
            "invalid_session_enumerate_limit",
            "session.enumerate limit must be an integer >= 1",
        ));
    };
    usize::try_from(limit).map(Some).map_err(|_| {
        invalid_request(
            "invalid_session_enumerate_limit",
            "session.enumerate limit is too large for this platform",
        )
    })
}

fn optional_bool(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<bool, ProviderFailure> {
    object
        .get(key)
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                invalid_request(
                    format!("invalid_session_enumerate_{key}"),
                    format!("session.enumerate {key} must be a boolean"),
                )
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(false))
}

fn optional_since_unix_ms(
    object: &serde_json::Map<String, Value>,
) -> Result<Option<i64>, ProviderFailure> {
    match object.get("since_unix_ms") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(|| {
            invalid_request(
                "invalid_session_enumerate_since_unix_ms",
                "session.enumerate since_unix_ms must be an integer or null",
            )
        }),
    }
}

fn enumerate_home(
    home: &Path,
    params: &EnumerateParams,
    bounds: ScanBounds,
) -> Result<SessionEnumerateResult, ProviderFailure> {
    let projects = claude_projects_dir(home);
    let scan = scan_transcript_files(home, bounds).map_err(session_enumerate_failed)?;
    let mut warnings = scan_warnings(&scan, bounds);
    let mut sessions = collect_sessions(&projects, params, bounds, scan.paths, &mut warnings);
    sort_sessions(&mut sessions);
    apply_limit(&mut sessions, params.limit);
    Ok(SessionEnumerateResult {
        sessions,
        complete: true,
        next_cursor: None,
        warnings,
    })
}

fn session_enumerate_failed(error: std::io::Error) -> ProviderFailure {
    failed(
        "session_enumerate_failed",
        format!("failed to scan Claude transcripts: {error}"),
    )
}

fn format_response_failed(error: serde_json::Error) -> ProviderFailure {
    failed(
        "session_enumerate_format_failed",
        format!("failed to format session.enumerate response: {error}"),
    )
}

fn scan_warnings(scan: &TranscriptScan, bounds: ScanBounds) -> Vec<String> {
    let mut warnings = Vec::new();
    if scan.truncated_depth {
        warnings.push(format!(
            "Claude projects scan truncated at max_depth={}",
            bounds.max_depth
        ));
    }
    if scan.truncated_entries {
        warnings.push(format!(
            "Claude projects scan truncated at max_entries={}",
            bounds.max_entries
        ));
    }
    warnings.extend(
        scan.skipped_confined_paths
            .iter()
            .map(|path| format!("skipped unconfined Claude transcript path: {path}")),
    );
    warnings
}

fn collect_sessions(
    projects: &Path,
    params: &EnumerateParams,
    bounds: ScanBounds,
    paths: Vec<std::path::PathBuf>,
    warnings: &mut Vec<String>,
) -> Vec<SessionEntry> {
    let mut by_id = HashMap::new();
    for path in paths {
        if let Some(entry) = session_entry_from_path(projects, params, bounds, &path, warnings) {
            insert_session_entry(&mut by_id, entry, warnings);
        }
    }
    by_id.into_values().collect()
}

fn session_entry_from_path(
    projects: &Path,
    params: &EnumerateParams,
    bounds: ScanBounds,
    path: &Path,
    warnings: &mut Vec<String>,
) -> Option<SessionEntry> {
    if let Err(error) = require_jsonl_transcript(path) {
        warnings.push(format!(
            "skipped non-transcript Claude projects entry: {}",
            transcript_path_error_text(error)
        ));
        return None;
    }
    let Some(provider_session_id) = provider_session_id(path) else {
        warnings.push(format!(
            "skipped Claude transcript with empty filename stem: {}",
            path.display()
        ));
        return None;
    };
    let file_updated_unix_ms = file_modified_unix_ms(path);
    let metadata = match read_metadata_records(path, bounds) {
        Ok(metadata) => metadata,
        Err(error) => {
            warnings.push(format!(
                "skipped malformed or unreadable Claude transcript {}: {error}",
                path.display()
            ));
            return None;
        }
    };
    if !passes_since(
        metadata_updated_unix_ms(&metadata, file_updated_unix_ms),
        params.since_unix_ms,
    ) {
        return None;
    }
    Some(SessionEntry {
        provider_session_id: provider_session_id.clone(),
        title: title_from_records(&metadata.records),
        cwd: cwd_from_records(params.include_cwd, &metadata.records),
        created_unix_ms: first_record_timestamp_unix_ms(&metadata.records),
        updated_unix_ms: metadata_updated_unix_ms(&metadata, file_updated_unix_ms),
        turn_count: turn_count_from_records(
            params.include_turn_count,
            metadata.truncated,
            &provider_session_id,
            &metadata.records,
            warnings,
        ),
        source: SessionSource {
            kind: "claude.projects_scan",
            detail: source_detail(projects, path),
        },
    })
}

fn transcript_path_error_text(error: TranscriptPathError) -> String {
    error.to_string()
}

fn provider_session_id(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
}

fn read_metadata_records(path: &Path, bounds: ScanBounds) -> Result<MetadataRecords, String> {
    let mut bounded = bounded_transcript_text(path, metadata_byte_limit(bounds))?;
    if bounded.truncated {
        truncate_to_last_newline(&mut bounded.text);
    }
    let (records, truncated_lines) = parse_metadata_lines(&bounded.text, bounds)?;
    Ok(MetadataRecords {
        records,
        truncated: bounded.truncated || truncated_lines,
    })
}

fn metadata_byte_limit(bounds: ScanBounds) -> u64 {
    const BYTES_PER_SCAN_ENTRY: u64 = 1024;
    const MIN_METADATA_BYTES: u64 = 4096;
    const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;

    (bounds.max_entries.max(1) as u64)
        .saturating_mul(BYTES_PER_SCAN_ENTRY)
        .clamp(MIN_METADATA_BYTES, MAX_METADATA_BYTES)
}

fn bounded_transcript_text(path: &Path, max_bytes: u64) -> Result<BoundedTranscriptText, String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let truncated = bytes.len() as u64 > max_bytes;
    if truncated {
        bytes.truncate(max_bytes as usize);
    }
    Ok(BoundedTranscriptText {
        text: bytes_to_string(bytes, truncated)?,
        truncated,
    })
}

fn bytes_to_string(mut bytes: Vec<u8>, allow_incomplete_tail: bool) -> Result<String, String> {
    loop {
        match String::from_utf8(bytes) {
            Ok(text) => return Ok(text),
            Err(error) if allow_incomplete_tail && error.utf8_error().error_len().is_none() => {
                let valid_up_to = error.utf8_error().valid_up_to();
                bytes = error.into_bytes();
                bytes.truncate(valid_up_to);
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

fn truncate_to_last_newline(text: &mut String) {
    if text.ends_with('\n') {
        return;
    }
    if let Some(index) = text.rfind('\n') {
        text.truncate(index + 1);
    } else {
        text.clear();
    }
}

fn parse_metadata_lines(text: &str, bounds: ScanBounds) -> Result<(Vec<Value>, bool), String> {
    let max_lines = bounds.max_entries.max(1);
    let mut records = Vec::new();
    let mut truncated = false;
    for (index, line) in text.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = serde_json::from_str::<Value>(trimmed)
            .map_err(|error| format!("malformed JSONL at line {}: {error}", index + 1))?;
        records.push(value);
    }
    Ok((records, truncated))
}

fn passes_since(updated_unix_ms: Option<i64>, since_unix_ms: Option<i64>) -> bool {
    since_unix_ms.is_none_or(|since| updated_unix_ms.is_some_and(|updated| updated >= since))
}

fn title_from_records(records: &[Value]) -> Option<String> {
    string_field_from_records(records, &["title", "summary"])
}

fn string_field_from_records(records: &[Value], keys: &[&str]) -> Option<String> {
    records.iter().find_map(|record| string_field(record, keys))
}

fn string_field(record: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| record.get(*key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn cwd_from_records(include_cwd: bool, records: &[Value]) -> Option<String> {
    if !include_cwd {
        return None;
    }
    records.iter().find_map(cwd_from_record)
}

fn cwd_from_record(record: &Value) -> Option<String> {
    record
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| Path::new(cwd).is_absolute())
        .map(str::to_string)
}

fn first_record_timestamp_unix_ms(records: &[Value]) -> Option<i64> {
    records.iter().find_map(record_timestamp_unix_ms)
}

fn metadata_updated_unix_ms(
    metadata: &MetadataRecords,
    file_updated_unix_ms: Option<i64>,
) -> Option<i64> {
    last_record_timestamp_unix_ms(&metadata.records).or(file_updated_unix_ms)
}

fn last_record_timestamp_unix_ms(records: &[Value]) -> Option<i64> {
    records.iter().rev().find_map(record_timestamp_unix_ms)
}

fn record_timestamp_unix_ms(record: &Value) -> Option<i64> {
    record
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_timestamp_unix_ms)
}

fn parse_timestamp_unix_ms(timestamp: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| timestamp.timestamp_millis())
}

fn file_modified_unix_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn turn_count_from_records(
    include_turn_count: bool,
    truncated: bool,
    provider_session_id: &str,
    records: &[Value],
    warnings: &mut Vec<String>,
) -> Option<usize> {
    if !include_turn_count {
        return None;
    }
    if truncated {
        warnings.push(format!(
            "turn_count unavailable for {provider_session_id}: transcript exceeded metadata read bounds"
        ));
        return None;
    }
    Some(
        records
            .iter()
            .filter(|record| is_counted_turn(record))
            .count(),
    )
}

fn is_counted_turn(record: &Value) -> bool {
    matches!(
        record.get("type").and_then(Value::as_str),
        Some("user" | "assistant")
    ) && record.get("isSidechain").and_then(Value::as_bool) != Some(true)
}

fn source_detail(projects: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(projects)
        .ok()
        .map(|path| path.display().to_string())
        .or_else(|| Some(path.display().to_string()))
}

fn insert_session_entry(
    by_id: &mut HashMap<String, SessionEntry>,
    entry: SessionEntry,
    warnings: &mut Vec<String>,
) {
    let id = entry.provider_session_id.clone();
    if let Some(existing) = by_id.get(&id) {
        let keep_new = prefer_new_entry(&entry, existing);
        warnings.push(duplicate_warning(existing, &entry, keep_new));
        if keep_new {
            by_id.insert(id, entry);
        }
    } else {
        by_id.insert(id, entry);
    }
}

fn prefer_new_entry(new: &SessionEntry, existing: &SessionEntry) -> bool {
    match compare_updated_desc(new.updated_unix_ms, existing.updated_unix_ms) {
        Ordering::Less => true,
        Ordering::Greater => false,
        Ordering::Equal => source_detail_text(new) < source_detail_text(existing),
    }
}

fn duplicate_warning(existing: &SessionEntry, new: &SessionEntry, keep_new: bool) -> String {
    let kept = if keep_new { new } else { existing };
    format!(
        "duplicate provider_session_id {}; kept {}; skipped duplicate {}",
        existing.provider_session_id,
        source_detail_text(kept),
        source_detail_text(if keep_new { existing } else { new })
    )
}

fn sort_sessions(sessions: &mut [SessionEntry]) {
    sessions.sort_by(|left, right| {
        compare_updated_desc(left.updated_unix_ms, right.updated_unix_ms)
            .then_with(|| left.provider_session_id.cmp(&right.provider_session_id))
            .then_with(|| source_detail_text(left).cmp(&source_detail_text(right)))
    });
}

fn compare_updated_desc(left: Option<i64>, right: Option<i64>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn source_detail_text(entry: &SessionEntry) -> String {
    entry.source.detail.clone().unwrap_or_default()
}

fn apply_limit(sessions: &mut Vec<SessionEntry>, limit: Option<usize>) {
    if let Some(limit) = limit {
        sessions.truncate(limit);
    }
}
