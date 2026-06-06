// declared_role: formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    validate_params(&request.params)?;
    let store = super::store::SettingsStore::for_host(&request.host)?;
    let records = store.list()?;
    Ok(list_response(&records))
}

fn validate_params(value: &Value) -> Result<(), ProviderFailure> {
    let params = super::validate::required_object(value, "settings.list")?;
    if params.is_empty() {
        return Ok(());
    }
    Err(super::validate::invalid_params("settings.list"))
}

fn list_response(records: &[super::store::SettingsRecord]) -> Value {
    let records = records
        .iter()
        .map(super::summary::public_summary)
        .collect::<Vec<_>>();
    json!({ "records": records })
}
