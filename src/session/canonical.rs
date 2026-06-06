// declared_role: accessor, filter, formatter, parser, predicate, validator

use serde_json::{json, Value};

use crate::envelope::error::ProviderFailure;

pub fn canonical_format_id() -> &'static str {
    "oulipoly.canonical_transcript/v1"
}

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalRecord {
    pub record_type: String,
    pub id: String,
    pub role: String,
    pub timestamp: String,
    pub body: Value,
}

pub fn parse_canonical(bytes: &[u8]) -> Result<Vec<CanonicalRecord>, ProviderFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| {
        ProviderFailure::invalid_request(
            "invalid_canonical_utf8",
            "canonical transcript bytes must be UTF-8 JSONL",
        )
    })?;
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let Some(trimmed) = non_empty_line(line) else {
            continue;
        };
        let value = parse_canonical_line(trimmed, line_number)?;
        records.push(parse_record(value, line_number)?);
    }
    Ok(records)
}

fn non_empty_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn parse_canonical_line(line: &str, line_number: usize) -> Result<Value, ProviderFailure> {
    serde_json::from_str::<Value>(line).map_err(|error| invalid_canonical_jsonl(line_number, error))
}

fn invalid_canonical_jsonl(line_number: usize, error: serde_json::Error) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_canonical_jsonl",
        format!("canonical transcript line {line_number} is invalid JSON: {error}"),
    )
}

pub fn serialize_canonical(records: &[CanonicalRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    for record in records {
        let value = json!({
            "body": record.body,
            "id": record.id,
            "role": record.role,
            "timestamp": record.timestamp,
            "type": record.record_type,
        });
        out.extend_from_slice(value.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

pub fn turn_count(records: &[CanonicalRecord]) -> usize {
    turn_records(records).len()
}

fn turn_records(records: &[CanonicalRecord]) -> Vec<&CanonicalRecord> {
    records.iter().filter(|record| is_turn(record)).collect()
}

fn is_turn(record: &CanonicalRecord) -> bool {
    record.record_type == "turn"
}

fn parse_record(value: Value, line_number: usize) -> Result<CanonicalRecord, ProviderFailure> {
    let fields = canonical_record_fields(&value, line_number)?;
    validate_record_type(&fields.record_type, line_number)?;
    validate_record_body(&fields.body, line_number)?;
    Ok(canonical_record(fields))
}

struct CanonicalRecordFields {
    record_type: String,
    id: String,
    role: String,
    timestamp: String,
    body: Value,
}

fn canonical_record_fields(
    value: &Value,
    line_number: usize,
) -> Result<CanonicalRecordFields, ProviderFailure> {
    Ok(CanonicalRecordFields {
        record_type: required_field(value, "type", line_number)?,
        id: required_field(value, "id", line_number)?,
        role: required_field(value, "role", line_number)?,
        timestamp: required_field(value, "timestamp", line_number)?,
        body: required_body(value, line_number)?,
    })
}

fn validate_record_type(record_type: &str, line_number: usize) -> Result<(), ProviderFailure> {
    if matches!(record_type, "turn" | "compaction_boundary") {
        Ok(())
    } else {
        Err(unsupported_canonical_record(line_number))
    }
}

fn unsupported_canonical_record(line_number: usize) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "unsupported_canonical_record",
        format!("canonical transcript line {line_number} has unsupported type"),
    )
}

fn required_body(value: &Value, line_number: usize) -> Result<Value, ProviderFailure> {
    value
        .get("body")
        .cloned()
        .ok_or_else(|| missing_canonical_body(line_number))
}

fn missing_canonical_body(line_number: usize) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_canonical_record",
        format!("canonical transcript line {line_number} missing body"),
    )
}

fn validate_record_body(body: &Value, line_number: usize) -> Result<(), ProviderFailure> {
    if body.is_array() {
        Ok(())
    } else {
        Err(invalid_canonical_body(line_number))
    }
}

fn invalid_canonical_body(line_number: usize) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_canonical_record",
        format!("canonical transcript line {line_number} body must be an array"),
    )
}

fn canonical_record(fields: CanonicalRecordFields) -> CanonicalRecord {
    CanonicalRecord {
        record_type: fields.record_type,
        id: fields.id,
        role: fields.role,
        timestamp: fields.timestamp,
        body: fields.body,
    }
}

fn required_field(value: &Value, key: &str, line_number: usize) -> Result<String, ProviderFailure> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|field| !field.is_empty())
        .map(str::to_string)
        .ok_or_else(|| missing_canonical_field(line_number, key))
}

fn missing_canonical_field(line_number: usize, key: &str) -> ProviderFailure {
    ProviderFailure::invalid_request(
        "invalid_canonical_record",
        format!("canonical transcript line {line_number} missing non-empty {key}"),
    )
}
