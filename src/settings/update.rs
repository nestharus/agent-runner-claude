// declared_role: formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let (id, version, values, diagnostics) = super::validate::for_update(&request.params)?;
    let store = super::store::SettingsStore::for_host(&request.host)?;
    let record = store.update(&id, &version, values)?;
    Ok(update_response(&record, diagnostics))
}

fn update_response(record: &super::store::SettingsRecord, diagnostics: Vec<Value>) -> Value {
    json!({
        "record": super::summary::public_record(record),
        "diagnostics": diagnostics
    })
}
