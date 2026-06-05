// declared_role: orchestration, validator, parser, accessor

use serde_json::Value;

pub fn decode_stdin_payload(payload: &Value) -> Result<Vec<u8>, String> {
    let object = validate_stdin_payload_object(payload)?;
    let (encoding, data) = stdin_payload_fields(object)?;
    decode_payload_bytes(encoding, data)
}

fn validate_stdin_payload_object(
    payload: &Value,
) -> Result<&serde_json::Map<String, Value>, String> {
    let object = payload
        .as_object()
        .ok_or_else(|| "stdin payload must be an object".to_string())?;
    if object.len() != 2 {
        return Err("stdin payload has unsupported fields".to_string());
    }

    Ok(object)
}

fn stdin_payload_fields(object: &serde_json::Map<String, Value>) -> Result<(&str, &str), String> {
    let encoding = object
        .get("encoding")
        .and_then(Value::as_str)
        .ok_or_else(|| "stdin payload missing encoding".to_string())?;
    let data = object
        .get("data")
        .and_then(Value::as_str)
        .ok_or_else(|| "stdin payload missing data".to_string())?;

    Ok((encoding, data))
}

fn decode_payload_bytes(encoding: &str, data: &str) -> Result<Vec<u8>, String> {
    match encoding {
        "base64" => crate::encoding::decode_base64(data),
        "utf8" => Ok(data.as_bytes().to_vec()),
        other => Err(format!("unsupported stdin payload encoding: {other}")),
    }
}
