// declared_role: formatter, mapper, predicate

use serde_json::{json, Value};

use super::store::SettingsRecord;

pub fn public_record(record: &SettingsRecord) -> Value {
    json!({
        "id": record.id,
        "display_name": record.display_name,
        "version": record.version,
        "values": redact_values(&record.values)
    })
}

pub fn public_summary(record: &SettingsRecord) -> Value {
    json!({
        "id": record.id,
        "display_name": record.display_name,
        "version": record.version,
        "summary": record_summary(&record.values)
    })
}

pub fn record_summary(values: &Value) -> Value {
    json!({
        "command": values.get("command").and_then(Value::as_str).unwrap_or("claude"),
        "has_credentials": has_any_secret(values),
        "setup_brain_model": values
            .get("setup_brain_model")
            .and_then(Value::as_str)
            .unwrap_or("claude-sonnet-4-6")
    })
}

fn has_any_secret(value: &Value) -> bool {
    match value {
        Value::Object(map) => map
            .iter()
            .any(|(key, value)| is_secret_key(key) || has_any_secret(value)),
        Value::Array(items) => items.iter().any(has_any_secret),
        _ => false,
    }
}

pub fn redact_values(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let redacted = map
                .iter()
                .map(|(key, value)| {
                    if is_secret_key(key) {
                        (key.clone(), redacted_secret())
                    } else {
                        (key.clone(), redact_values(value))
                    }
                })
                .collect();
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_values).collect()),
        _ => value.clone(),
    }
}

fn redacted_secret() -> Value {
    json!("[redacted]")
}

fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("api_key")
        || key.contains("auth_token")
        || key.contains("token")
        || key.contains("secret")
        || key.contains("credential")
        || key.contains("password")
}
