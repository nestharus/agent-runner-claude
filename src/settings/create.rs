// declared_role: formatter, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let (display_name, values, diagnostics) = super::validate::for_create(&request.params)?;
    let store = super::store::SettingsStore::for_host(&request.host)?;
    let record = store.create(display_name.as_deref(), values)?;
    Ok(create_response(&record, diagnostics))
}

fn create_response(record: &super::store::SettingsRecord, diagnostics: Vec<Value>) -> Value {
    json!({
        "record": super::summary::public_record(record),
        "diagnostics": diagnostics
    })
}
