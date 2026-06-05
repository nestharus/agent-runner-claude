// declared_role: parser, validator, mapper

use serde_json::Value;

pub fn parse_quota_value(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}

pub fn parse_windows(stdout: &[u8]) -> Result<Vec<Value>, String> {
    let text =
        std::str::from_utf8(stdout).map_err(|_| "quota stdout must be UTF-8 JSON".to_string())?;
    let value =
        parse_quota_value(text).map_err(|error| format!("quota stdout must be JSON: {error}"))?;
    parse_value_windows(&value)
}

fn parse_value_windows(value: &Value) -> Result<Vec<Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "quota output must be a JSON object".to_string())?;

    if let Some(windows) = object.get("windows") {
        let windows = windows
            .as_array()
            .ok_or_else(|| "quota windows must be an array".to_string())?;
        return windows.iter().map(parse_window_object).collect();
    }

    parse_window_object(value).map(|window| vec![window])
}

fn parse_window_object(value: &Value) -> Result<Value, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "quota window must be an object".to_string())?;
    let name = object
        .get("name")
        .or_else(|| object.get("label"))
        .or_else(|| object.get("window"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let used_percent = object
        .get("used_percent")
        .and_then(Value::as_f64)
        .ok_or_else(|| "quota window missing numeric used_percent".to_string())?;
    let reset = object
        .get("resets_at_unix_ms")
        .or_else(|| object.get("resets_at"))
        .or_else(|| object.get("reset_timestamp"))
        .ok_or_else(|| "quota window missing reset timestamp".to_string())?;

    super::window::window_value(name, used_percent, reset)
}
