// declared_role: parser, predicate

use serde_json::Value;

// declared_role: parser
pub(crate) fn extract_stdout_session_id(stdout: &[u8]) -> Option<String> {
    stdout_json_values(stdout)
        .into_iter()
        .find_map(|value| stdout_capture_session_id(&value))
}

// declared_role: parser
fn stdout_json_values(stdout: &[u8]) -> Vec<Value> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().filter_map(parse_json_line).collect()
}

// declared_role: parser
fn stdout_capture_session_id(value: &Value) -> Option<String> {
    is_stdout_capture_event(value).then(|| stdout_capture_session_id_value(value))?
}

// declared_role: parser
fn stdout_capture_session_id_value(value: &Value) -> Option<String> {
    json_session_id(value).map(str::to_string)
}

// declared_role: parser
fn parse_json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line.trim()).ok()
}

// declared_role: parser
fn json_session_id(value: &Value) -> Option<&str> {
    value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
}

// declared_role: predicate
fn is_stdout_capture_event(value: &Value) -> bool {
    string_field_equals(value, "type", "system")
        && string_field_equals(value, "subtype", "init")
        && json_session_id(value).is_some()
}

// declared_role: predicate
fn string_field_equals(value: &Value, key: &str, expected: &str) -> bool {
    value.get(key).and_then(Value::as_str) == Some(expected)
}
