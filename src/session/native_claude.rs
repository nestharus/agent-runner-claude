// declared_role: accessor, filter, formatter, mapper, orchestration, parser, predicate

use serde_json::{json, Value};

use super::canonical::CanonicalRecord;

pub fn native_format_id() -> &'static str {
    "claude_code"
}

#[derive(Debug, Clone)]
pub struct NativeRecord {
    pub line_number: usize,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub struct NativeTurn {
    pub id: String,
    pub role: String,
    pub body: Value,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NativeParse {
    pub records: Vec<NativeRecord>,
    pub complete: bool,
}

pub fn parse_native_jsonl(text: &str) -> NativeParse {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_number = index + 1;
        let Some(trimmed) = non_empty_line(line) else {
            continue;
        };
        match parse_native_line(trimmed) {
            Ok(value) => records.push(native_record(line_number, value)),
            Err(_) => {
                return NativeParse {
                    records,
                    complete: false,
                };
            }
        }
    }
    NativeParse {
        records,
        complete: true,
    }
}

fn non_empty_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn parse_native_line(line: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str::<Value>(line)
}

fn native_record(line_number: usize, value: Value) -> NativeRecord {
    NativeRecord { line_number, value }
}

pub fn text_contains_session(text: &str, session_id: &str) -> bool {
    parse_native_jsonl(text).contains_session(session_id)
}

pub fn turns_for_session(parse: &NativeParse, session_id: &str) -> Vec<NativeTurn> {
    turn_records_for_session(parse, session_id)
        .into_iter()
        .map(turn_from_record)
        .collect()
}

pub fn canonical_records_for_session(
    parse: &NativeParse,
    session_id: &str,
) -> Vec<CanonicalRecord> {
    canonical_record_refs_for_session(parse, session_id)
        .into_iter()
        .map(canonical_from_record)
        .collect()
}

pub fn turn_from_record(record: &NativeRecord) -> NativeTurn {
    let record_type = record_type(&record.value).unwrap_or("user");
    let role = record
        .value
        .get("message")
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .unwrap_or(record_type)
        .to_string();

    NativeTurn {
        id: stable_id(&record.value, record.line_number),
        role,
        body: normalized_body(&record.value),
        timestamp: record_timestamp(record),
    }
}

pub fn canonical_from_record(record: &NativeRecord) -> CanonicalRecord {
    let timestamp = record
        .value
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if canonical_record_is_compaction(record) {
        CanonicalRecord {
            record_type: "compaction_boundary".to_string(),
            id: stable_id(&record.value, record.line_number),
            role: "summary".to_string(),
            timestamp,
            body: normalized_body(&record.value),
        }
    } else {
        canonical_turn(turn_from_record(record), timestamp)
    }
}

pub fn render_native_jsonl(session_id: &str, records: &[CanonicalRecord]) -> Vec<u8> {
    serialize_native_jsonl(&native_values(session_id, records))
}

impl NativeParse {
    fn contains_session(&self, session_id: &str) -> bool {
        self.records
            .iter()
            .any(|record| record_matches_session(&record.value, session_id))
    }
}

fn turn_records_for_session<'a>(parse: &'a NativeParse, session_id: &str) -> Vec<&'a NativeRecord> {
    parse
        .records
        .iter()
        .filter(|record| is_turn_record_for_session(record, session_id))
        .collect()
}

fn canonical_record_refs_for_session<'a>(
    parse: &'a NativeParse,
    session_id: &str,
) -> Vec<&'a NativeRecord> {
    parse
        .records
        .iter()
        .filter(|record| is_canonical_record_for_session(record, session_id))
        .collect()
}

fn is_turn_record_for_session(record: &NativeRecord, session_id: &str) -> bool {
    record_matches_session(&record.value, session_id)
        && !is_sidechain(&record.value)
        && is_turn_type(record_type(&record.value))
}

fn is_canonical_record_for_session(record: &NativeRecord, session_id: &str) -> bool {
    record_matches_session(&record.value, session_id)
        && !is_sidechain(&record.value)
        && (is_turn_type(record_type(&record.value)) || is_compaction_record(record))
}

fn is_turn_type(record_type: Option<&str>) -> bool {
    matches!(record_type, Some("user" | "assistant"))
}

fn is_compaction_record(record: &NativeRecord) -> bool {
    record
        .value
        .get("isCompactSummary")
        .and_then(Value::as_bool)
        == Some(true)
}

fn canonical_record_is_compaction(record: &NativeRecord) -> bool {
    is_compaction_record(record)
}

fn record_type(value: &Value) -> Option<&str> {
    value.get("type").and_then(Value::as_str)
}

fn record_timestamp(record: &NativeRecord) -> Option<String> {
    record
        .value
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn canonical_turn(turn: NativeTurn, timestamp: String) -> CanonicalRecord {
    CanonicalRecord {
        record_type: "turn".to_string(),
        id: turn.id,
        role: turn.role,
        timestamp,
        body: turn.body,
    }
}

fn native_values(session_id: &str, records: &[CanonicalRecord]) -> Vec<Value> {
    records
        .iter()
        .map(|record| native_value(session_id, record))
        .collect()
}

fn serialize_native_jsonl(values: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        out.extend_from_slice(value.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

fn native_value(session_id: &str, record: &CanonicalRecord) -> Value {
    if canonical_record_type_is_compaction(record) {
        native_compaction_value(session_id, record)
    } else {
        native_turn_value(session_id, record)
    }
}

fn canonical_record_type_is_compaction(record: &CanonicalRecord) -> bool {
    record.record_type == "compaction_boundary"
}

fn native_compaction_value(session_id: &str, record: &CanonicalRecord) -> Value {
    json!({
        "sessionId": session_id,
        "uuid": uuid_from_canonical_id(&record.id),
        "timestamp": record.timestamp,
        "type": "system",
        "isCompactSummary": true,
        "message": { "content": native_content(&record.body) }
    })
}

fn native_turn_value(session_id: &str, record: &CanonicalRecord) -> Value {
    json!({
        "sessionId": session_id,
        "uuid": uuid_from_canonical_id(&record.id),
        "timestamp": record.timestamp,
        "type": record.role,
        "message": { "role": record.role, "content": native_content(&record.body) }
    })
}

fn record_matches_session(value: &Value, session_id: &str) -> bool {
    value.get("sessionId").and_then(Value::as_str) == Some(session_id)
        || value.get("session_id").and_then(Value::as_str) == Some(session_id)
}

fn is_sidechain(value: &Value) -> bool {
    value.get("isSidechain").and_then(Value::as_bool) == Some(true)
}

fn stable_id(value: &Value, line_number: usize) -> String {
    value
        .get("uuid")
        .and_then(Value::as_str)
        .filter(|uuid| !uuid.is_empty())
        .map(|uuid| format!("uuid:{uuid}"))
        .unwrap_or_else(|| format!("line:{line_number}"))
}

fn normalized_body(value: &Value) -> Value {
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"));
    normalize_content(content.unwrap_or(&Value::Null))
}

fn normalize_content(content: &Value) -> Value {
    match content {
        Value::String(text) => json!([{ "type": "text", "text": text }]),
        Value::Array(_) => content.clone(),
        Value::Object(_) => json!([content.clone()]),
        Value::Null => json!([]),
        other => normalized_scalar_content(other),
    }
}

fn normalized_scalar_content(value: &Value) -> Value {
    json!([{ "type": "text", "text": value.to_string() }])
}

fn native_content(body: &Value) -> Value {
    let Some(items) = body.as_array() else {
        return body.clone();
    };
    if let Some(text) = single_text_item(items) {
        return native_text_content(text);
    }
    body.clone()
}

fn single_text_item(items: &[Value]) -> Option<&str> {
    if items.len() != 1 {
        return None;
    }
    text_item_value(&items[0])
}

fn text_item_value(item: &Value) -> Option<&str> {
    if item.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    item.get("text").and_then(Value::as_str)
}

fn native_text_content(text: &str) -> Value {
    Value::String(text.to_string())
}

fn uuid_from_canonical_id(id: &str) -> String {
    id.strip_prefix("uuid:").unwrap_or(id).to_string()
}
