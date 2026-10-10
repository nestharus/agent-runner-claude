//! Shared SDK definitions admit the base boundary; Claude interprets admitted
//! inputs. No provider-private schema snapshot or operation schema lives here.

use crate::{ProviderFailure, RequestEnvelope};
use agent_provider_contract::schemas::{schema_by_file, SchemaRegistry};
use jsonschema::{Draft, JSONSchema};
use serde_json::{json, Value};
use std::sync::OnceLock;

pub(crate) fn envelope(value: &Value) -> Result<(), String> {
    // The SDK operation registry intentionally has no resident.prepare row.
    // Its base envelope still applies to extensions and unsupported commands.
    // Select that definition directly from the SDK, using the existing engine.
    static ENVELOPE: OnceLock<JSONSchema> = OnceLock::new();
    let validator = ENVELOPE.get_or_init(|| {
        let common: Value =
            serde_json::from_str(schema_by_file("common.schema.json").expect("SDK common schema"))
                .expect("SDK schema JSON");
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$defs": common["$defs"],
            "$ref": "#/$defs/RequestEnvelope"
        });
        JSONSchema::options()
            .with_draft(Draft::Draft202012)
            .compile(&schema)
            .expect("SDK base envelope schema")
    });
    validator.validate(value).map_err(|errors| {
        errors
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    })
}

pub(crate) fn operation(
    subcommand: &str,
    request: &RequestEnvelope,
) -> Result<(), ProviderFailure> {
    // Unimplemented commands are refused, not admitted for execution. The
    // resident extension keeps its SDK-owned params admission in prepare.
    if !matches!(
        subcommand,
        "describe"
            | "schema"
            | "policy.evaluate"
            | "launch"
            | "terminal.classify"
            | "quota.source"
            | "quota.probe"
            | "quota.refresh_auth"
            | "session.locate_transcript"
            | "session.capture"
            | "session.export"
            | "session.replace"
    ) {
        return Ok(());
    }
    let value = serde_json::to_value(request).expect("SDK request serializes");
    SchemaRegistry::new()
        .validate_request(subcommand, &value)
        .map_err(|error| {
            ProviderFailure::invalid_request(
                request.request_id.clone(),
                "invalid_params",
                error.to_string(),
            )
        })
}
