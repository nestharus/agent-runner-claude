// declared_role: orchestration, accessor, validator, mapper, parser
// adapter_declarations:
//   - component: src/launch/params.rs
//     role: adapter
//     Translates:
//       - contract/v1/launch.schema.json#/$defs/LaunchRequest
//       - contract/v1/launch.schema.json#/$defs/LaunchParams
//       - contract/v1/common.schema.json#/$defs/BytePayload

use serde_json::Value;

pub fn params_value(value: &Value) -> &Value {
    value
}

pub fn required_string<'a>(params: &'a Value, field: &str) -> Result<&'a str, String> {
    require_non_empty_string(string_field(params, field), field)
}

pub fn argv(params: &Value) -> Result<Vec<String>, String> {
    let values = argv_array(params)?;
    require_program(values)?;
    let strings = argv_strings(values)?;
    Ok(owned_strings(&strings))
}

fn string_field<'a>(params: &'a Value, field: &str) -> Option<&'a str> {
    params.get(field).and_then(Value::as_str)
}

fn require_non_empty_string<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, String> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("launch params missing string field: {field}"))
}

fn argv_array(params: &Value) -> Result<&Vec<Value>, String> {
    params
        .get("argv")
        .and_then(Value::as_array)
        .ok_or_else(|| "launch params missing argv".to_string())
}

fn require_program(values: &[Value]) -> Result<(), String> {
    if values.is_empty() {
        return Err("launch argv must contain a program".to_string());
    }

    Ok(())
}

fn argv_strings(values: &[Value]) -> Result<Vec<&str>, String> {
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "launch argv entries must be strings".to_string())
        })
        .collect()
}

fn owned_strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}
