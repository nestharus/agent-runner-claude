// declared_role: orchestration, mapper, parser, validator, formatter, accessor, filter, predicate
// intrinsic_surface_declarations:
//   - component: tests/support/schema.rs
//     role: intrinsic-surface
//     Domain: contract_schema_validation_bundle
//     Owns:
//       - bundled contract/v1 schema document set
//       - schema reference rewrite and validation harness surface

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
    let compiled = compiled_contract_schema(schema_id_or_path);
    validate_value(&compiled, value)
}

fn compiled_contract_schema(schema_id_or_path: &str) -> JSONSchema {
    compile_contract_schema(
        schema_id_or_path,
        &bundled_contract_schema(schema_id_or_path),
    )
}

fn compile_contract_schema(schema_id_or_path: &str, root: &Value) -> JSONSchema {
    JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(root)
        .unwrap_or_else(|err| panic!("compile contract schema {schema_id_or_path}: {err}"))
}

fn validate_value(compiled: &JSONSchema, value: &Value) -> Result<(), Vec<String>> {
    compiled.validate(value).map_err(formatted_errors)
}

fn formatted_errors<'a>(
    errors: impl Iterator<Item = jsonschema::error::ValidationError<'a>>,
) -> Vec<String> {
    errors.map(validation_error_message).collect::<Vec<_>>()
}

fn validation_error_message(error: jsonschema::error::ValidationError<'_>) -> String {
    format!("{} at {}", error, error.instance_path)
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
    merge_contract_schema_docs(parsed_bundled_schemas())
}

fn parsed_bundled_schemas() -> Vec<Value> {
    SCHEMAS
        .iter()
        .map(|(_, text)| parsed_contract_schema(text))
        .collect()
}

fn merge_contract_schema_docs(docs: Vec<Value>) -> Map<String, Value> {
    let mut defs = Map::new();
    for doc in docs {
        merge_schema_defs(&mut defs, &doc);
    }
    defs
}

fn parsed_contract_schema(text: &str) -> Value {
    let mut doc = parse_contract_schema_json(text);
    rewrite_external_refs(&mut doc);
    doc
}

fn parse_contract_schema_json(text: &str) -> Value {
    serde_json::from_str::<Value>(text).expect("schema JSON must parse")
}

fn merge_schema_defs(defs: &mut Map<String, Value>, doc: &Value) {
    if let Some(schema_defs) = schema_defs(doc) {
        extend_defs(defs, schema_defs);
    }
}

fn schema_defs(doc: &Value) -> Option<&Map<String, Value>> {
    doc.get("$defs").and_then(Value::as_object)
}

fn extend_defs(defs: &mut Map<String, Value>, schema_defs: &Map<String, Value>) {
    defs.extend(cloned_def_entries(schema_defs));
}

fn cloned_def_entries(
    schema_defs: &Map<String, Value>,
) -> impl Iterator<Item = (String, Value)> + '_ {
    schema_defs
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
}

fn contract_schema_wrapper(def_name: String, defs: Map<String, Value>) -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs,
        "$ref": format!("#/$defs/{def_name}")
    })
}

fn def_name_for(schema_id_or_path: &str, defs: &Map<String, Value>) -> String {
    if let Some(def_name) = ref_def_name(schema_id_or_path) {
        return def_name.to_string();
    }
    if defs_contains_name(defs, schema_id_or_path) {
        return schema_id_or_path.to_string();
    }
    panic!("schema reference must name a $defs entry: {schema_id_or_path}");
}

fn ref_def_name(schema_id_or_path: &str) -> Option<&str> {
    schema_id_or_path
        .split_once("#/$defs/")
        .map(|(_, def_name)| def_name)
}

fn defs_contains_name(defs: &Map<String, Value>, name: &str) -> bool {
    defs.contains_key(name)
}

fn rewrite_external_refs(value: &mut Value) {
    match value {
        Value::Object(map) => rewrite_object_external_refs(map),
        Value::Array(items) => rewrite_array_external_refs(items),
        _ => {}
    }
}

fn rewrite_object_external_refs(map: &mut Map<String, Value>) {
    rewrite_ref_field(map);
    for child in map.values_mut() {
        rewrite_external_refs(child);
    }
}

fn rewrite_ref_field(map: &mut Map<String, Value>) {
    if let Some(Value::String(reference)) = map.get_mut("$ref") {
        rewrite_reference(reference);
    }
}

fn rewrite_reference(reference: &mut String) {
    if let Some(def_path) = external_schema_def_path(reference) {
        *reference = internal_schema_ref(def_path);
    }
}

fn external_schema_def_path(reference: &str) -> Option<&str> {
    let (document, def_path) = schema_ref_parts(reference)?;
    is_external_schema_document(document).then_some(def_path)
}

fn internal_schema_ref(def_path: &str) -> String {
    format!("#/$defs/{def_path}")
}

fn schema_ref_parts(reference: &str) -> Option<(&str, &str)> {
    reference.split_once("#/$defs/")
}

fn is_external_schema_document(document: &str) -> bool {
    document.ends_with(".schema.json")
}

fn rewrite_array_external_refs(items: &mut [Value]) {
    for item in items {
        rewrite_external_refs(item);
    }
}
