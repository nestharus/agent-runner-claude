// declared_role: formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let id = validate_params(&request.params)?;
    let store = super::store::SettingsStore::for_host(&request.host)?;
    let record = store.get(id)?;
    Ok(get_response(&record))
}

fn validate_params(value: &Value) -> Result<&str, ProviderFailure> {
    let params = super::validate::required_object(value, "settings.get")?;
    if params.len() != 1 || params.get("id").and_then(Value::as_str).is_none() {
        return Err(super::validate::invalid_params("settings.get"));
    }
    Ok(params["id"].as_str().unwrap())
}

fn get_response(record: &super::store::SettingsRecord) -> Value {
    json!({ "record": super::summary::public_record(record) })
}
