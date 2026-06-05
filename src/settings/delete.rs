// declared_role: formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let (id, version) = validate_params(&request.params)?;
    let store = super::store::SettingsStore::for_host(&request.host)?;
    let deleted = store.delete(id, version)?;
    Ok(delete_response(id, deleted))
}

fn validate_params(value: &Value) -> Result<(&str, &str), ProviderFailure> {
    let params = super::validate::required_object(value, "settings.delete")?;
    if params.len() != 2
        || params.get("id").and_then(Value::as_str).is_none()
        || params.get("version").and_then(Value::as_str).is_none()
    {
        return Err(super::validate::invalid_params("settings.delete"));
    }
    Ok((
        params["id"].as_str().unwrap(),
        params["version"].as_str().unwrap(),
    ))
}

fn delete_response(id: &str, deleted: bool) -> Value {
    json!({ "deleted": deleted, "id": id })
}
