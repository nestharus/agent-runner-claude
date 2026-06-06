// declared_role: accessor, filter, formatter, mapper, orchestration, validator

use serde_json::{json, Value};

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

struct PersistableAction<'a> {
    action: &'a Value,
    values: &'a Value,
}

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
    persist_actions_to_store(&store, persistable_actions(actions))
}

fn persist_actions_to_store(
    store: &super::store::SettingsStore,
    actions: Vec<PersistableAction<'_>>,
) -> Result<Vec<Value>, ProviderFailure> {
    let mut diagnostics = Vec::new();
    for action in actions {
        persist_action(store, action, &mut diagnostics)?;
    }
    Ok(diagnostics)
}

fn persistable_actions(actions: &[Value]) -> Vec<PersistableAction<'_>> {
    actions.iter().filter_map(persistable_action).collect()
}

fn persistable_action(action: &Value) -> Option<PersistableAction<'_>> {
    action_values(action).map(|values| PersistableAction { action, values })
}

fn persist_action(
    store: &super::store::SettingsStore,
    action: PersistableAction<'_>,
    diagnostics: &mut Vec<Value>,
) -> Result<(), ProviderFailure> {
    diagnostics.extend(action_diagnostics(action.values));
    create_settings_record(store, action.action, action.values)
}

fn action_values(action: &Value) -> Option<&Value> {
    action_values_value(action).filter(value_is_object)
}

fn action_values_value(action: &Value) -> Option<&Value> {
    action.get("values")
}

fn value_is_object(value: &&Value) -> bool {
    value.is_object()
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
    legacy_provider_entries(legacy)
        .into_iter()
        .map(|(name, values)| migration_action(name, values))
        .collect()
}

fn legacy_provider_entries(legacy: &Value) -> Vec<(&str, &Value)> {
    legacy_providers(legacy)
        .map(|providers| providers.iter().filter_map(legacy_provider_entry).collect())
        .unwrap_or_default()
}

fn legacy_providers(legacy: &Value) -> Option<&serde_json::Map<String, Value>> {
    legacy.get("providers.toml").and_then(Value::as_object)
}

fn legacy_provider_entry<'a>(entry: (&'a String, &'a Value)) -> Option<(&'a str, &'a Value)> {
    let (name, values) = entry;
    legacy_provider_values(values).map(|values| legacy_provider_entry_value(name, values))
}

fn legacy_provider_values(values: &Value) -> Option<&Value> {
    values.as_object().map(|_| values)
}

fn legacy_provider_entry_value<'a>(name: &'a str, values: &'a Value) -> (&'a str, &'a Value) {
    (name, values)
}

fn migration_action(name: &str, values: &Value) -> Value {
    json!({
        "kind": "create_settings_record",
        "source": "providers.toml",
        "display_name": name,
        "values": values
    })
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
