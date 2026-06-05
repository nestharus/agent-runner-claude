use jsonschema::{Draft, JSONSchema};
use serde_json::{json, Map, Value};

const SCHEMAS: &[(&str, &str)] = &[
    (
        "common.schema.json",
        include_str!("../../contract/v1/common.schema.json"),
    ),
    (
        "describe.schema.json",
        include_str!("../../contract/v1/describe.schema.json"),
    ),
    (
        "schema.schema.json",
        include_str!("../../contract/v1/schema.schema.json"),
    ),
    (
        "settings.schema.json",
        include_str!("../../contract/v1/settings.schema.json"),
    ),
    (
        "policy.schema.json",
        include_str!("../../contract/v1/policy.schema.json"),
    ),
    (
        "terminal.schema.json",
        include_str!("../../contract/v1/terminal.schema.json"),
    ),
    (
        "launch.schema.json",
        include_str!("../../contract/v1/launch.schema.json"),
    ),
    (
        "quota.schema.json",
        include_str!("../../contract/v1/quota.schema.json"),
    ),
    (
        "rotation.schema.json",
        include_str!("../../contract/v1/rotation.schema.json"),
    ),
    (
        "migration.schema.json",
        include_str!("../../contract/v1/migration.schema.json"),
    ),
    (
        "session.schema.json",
        include_str!("../../contract/v1/session.schema.json"),
    ),
    (
        "discovery.schema.json",
        include_str!("../../contract/v1/discovery.schema.json"),
    ),
    (
        "setup.schema.json",
        include_str!("../../contract/v1/setup.schema.json"),
    ),
];

pub fn validate(schema_id_or_path: &str, value: &Value) -> Result<(), Vec<String>> {
    let root = bundled_contract_schema(schema_id_or_path);
    let compiled = JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(&root)
        .unwrap_or_else(|err| panic!("compile contract schema {schema_id_or_path}: {err}"));

    compiled.validate(value).map_err(|errors| {
        errors
            .map(|error| format!("{} at {}", error, error.instance_path))
            .collect::<Vec<_>>()
    })
}

pub fn assert_valid(schema_id_or_path: &str, value: &Value) {
    if let Err(errors) = validate(schema_id_or_path, value) {
        panic!(
            "contract validation failed for {schema_id_or_path}:\n{}\nvalue:\n{}",
            errors.join("\n"),
            value
        );
    }
}

pub fn compile_arbitrary_schema(schema: &Value) -> Result<(), String> {
    JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(schema)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn bundled_contract_schema(schema_id_or_path: &str) -> Value {
    let defs = bundled_schema_defs();
    let def_name = def_name_for(schema_id_or_path, &defs);
    contract_schema_wrapper(def_name, defs)
}

fn bundled_schema_defs() -> Map<String, Value> {
    let mut defs = Map::new();
    for (_, text) in SCHEMAS {
        let doc = parsed_contract_schema(text);
        merge_schema_defs(&mut defs, &doc);
    }
    defs
}

fn parsed_contract_schema(text: &str) -> Value {
    let mut doc = serde_json::from_str::<Value>(text).expect("schema JSON must parse");
    rewrite_external_refs(&mut doc);
    doc
}

fn merge_schema_defs(defs: &mut Map<String, Value>, doc: &Value) {
    if let Some(schema_defs) = doc.get("$defs").and_then(Value::as_object) {
        defs.extend(
            schema_defs
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
}

fn contract_schema_wrapper(def_name: String, defs: Map<String, Value>) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs,
        "$ref": format!("#/$defs/{def_name}")
    })
}

fn def_name_for(schema_id_or_path: &str, defs: &Map<String, Value>) -> String {
    if let Some((_, def_name)) = schema_id_or_path.split_once("#/$defs/") {
        return def_name.to_string();
    }
    if defs.contains_key(schema_id_or_path) {
        return schema_id_or_path.to_string();
    }
    panic!("schema reference must name a $defs entry: {schema_id_or_path}");
}

fn rewrite_external_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(reference)) = map.get_mut("$ref") {
                if let Some((document, def_path)) = reference.split_once("#/$defs/") {
                    if document.ends_with(".schema.json") {
                        *reference = format!("#/$defs/{def_path}");
                    }
                }
            }
            for child in map.values_mut() {
                rewrite_external_refs(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_external_refs(item);
            }
        }
        _ => {}
    }
}
