// declared_role: orchestration, validator, mapper, parser, formatter

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

pub fn remaining_ratio(used_percent: f64) -> f64 {
    1.0 - used_percent / 100.0
}

pub fn window_value(
    name: Option<String>,
    used_percent: f64,
    reset_value: &Value,
) -> Result<Value, String> {
    validate_used_percent(used_percent)?;
    let remaining_ratio = remaining_ratio(used_percent);
    let resets_at_unix_ms = reset_to_unix_ms(reset_value)?;
    Ok(format_window_value(
        name,
        remaining_ratio,
        resets_at_unix_ms,
    ))
}

fn validate_used_percent(used_percent: f64) -> Result<(), String> {
    if !(0.0..=100.0).contains(&used_percent) || !used_percent.is_finite() {
        return Err("used_percent must be between 0 and 100".to_string());
    }

    Ok(())
}

fn format_window_value(
    name: Option<String>,
    remaining_ratio: f64,
    resets_at_unix_ms: u64,
) -> Value {
    let mut object = serde_json::Map::new();
    if let Some(name) = name {
        object.insert("name".to_string(), json!(name));
    }
    object.insert("remaining_ratio".to_string(), json!(remaining_ratio));
    object.insert("resets_at_unix_ms".to_string(), json!(resets_at_unix_ms));
    Value::Object(object)
}

fn reset_to_unix_ms(value: &Value) -> Result<u64, String> {
    if let Some(ms) = value.as_u64() {
        return Ok(if ms < 10_000_000_000 { ms * 1000 } else { ms });
    }
    if let Some(seconds) = value.as_i64() {
        return unix_ms_from_i64(seconds);
    }

    let text = value
        .as_str()
        .ok_or_else(|| "reset timestamp must be a number or RFC3339 string".to_string())?;
    unix_ms_from_rfc3339(text)
}

fn unix_ms_from_i64(seconds: i64) -> Result<u64, String> {
    if seconds < 0 {
        return Err("reset timestamp cannot be negative".to_string());
    }

    Ok(if seconds < 10_000_000_000 {
        seconds as u64 * 1000
    } else {
        seconds as u64
    })
}

fn unix_ms_from_rfc3339(text: &str) -> Result<u64, String> {
    let parsed = parse_rfc3339_reset(text)?;
    Ok(normalized_reset_unix_ms(parsed))
}

fn parse_rfc3339_reset(text: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(invalid_reset_timestamp)
}

fn normalized_reset_unix_ms(parsed: DateTime<Utc>) -> u64 {
    parsed.timestamp_millis().max(0) as u64
}

fn invalid_reset_timestamp(error: chrono::ParseError) -> String {
    format!("invalid reset timestamp: {error}")
}
