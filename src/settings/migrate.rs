// declared_role: filter, formatter, mapper, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn handle(request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    let (dry_run, legacy) = super::validate::for_migrate(&request.params)?;
    let actions = migration_actions(&legacy);
    let diagnostics = persist_actions(&request.host, dry_run, &actions)?;
    Ok(migration_response(actions, diagnostics))
}

fn persist_actions(
    host: &Value,
    dry_run: bool,
    actions: &[Value],
) -> Result<Vec<Value>, ProviderFailure> {
    if dry_run {
        return Ok(Vec::new());
    }
    let store = super::store::SettingsStore::for_host(host)?;
    let mut diagnostics = Vec::new();
    for action in actions {
        let Some(values) = action_values(action) else {
            continue;
        };
        diagnostics.extend(action_diagnostics(values));
        create_settings_record(&store, action, values)?;
    }
    Ok(diagnostics)
}

fn action_values(action: &Value) -> Option<&Value> {
    action.get("values").filter(|value| value.is_object())
}

fn action_diagnostics(values: &Value) -> Vec<Value> {
    super::validate::validate_values(values)
}

fn create_settings_record(
    store: &super::store::SettingsStore,
    action: &Value,
    values: &Value,
) -> Result<(), ProviderFailure> {
    let _ = store.create(action_display_name(action), values.clone())?;
    Ok(())
}

fn action_display_name(action: &Value) -> Option<&str> {
    action.get("display_name").and_then(Value::as_str)
}

fn migration_response(actions: Vec<Value>, diagnostics: Vec<Value>) -> Value {
    json!({
        "actions": actions.into_iter().map(redact_action).collect::<Vec<_>>(),
        "warnings": [],
        "requires_user_input": false,
        "diagnostics": diagnostics
    })
}

fn migration_actions(legacy: &Value) -> Vec<Value> {
    legacy
        .get("providers.toml")
        .and_then(Value::as_object)
        .map(|providers| {
            providers
                .iter()
                .filter_map(|(name, values)| {
                    values.as_object().map(|_| {
                        json!({
                            "kind": "create_settings_record",
                            "source": "providers.toml",
                            "display_name": name,
                            "values": values
                        })
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn redact_action(action: Value) -> Value {
    let Some(mut object) = action.as_object().cloned() else {
        return action;
    };
    if let Some(values) = object.get("values") {
        object.insert(
            "values".to_string(),
            crate::settings::summary::redact_values(values),
        );
    }
    Value::Object(object)
}
