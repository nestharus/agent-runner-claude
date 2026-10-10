//! Shared SDK definitions admit the base boundary; Claude interprets admitted
//! inputs. No provider-private schema snapshot or operation schema lives here.

use crate::{ProviderFailure, RequestEnvelope};
use agent_provider_contract::schemas::{schema_by_file, SchemaRegistry};
use jsonschema::{Draft, JSONSchema};
use serde_json::{json, Value};
use std::sync::OnceLock;

pub(crate) fn envelope(value: &Value) -> Result<(), &'static str> {
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
    // Engine diagnostics can contain entire submitted values. Only return the
    // trusted admission boundary, never the validator's value-bearing text.
    validator
        .validate(value)
        .map_err(|_errors| "request envelope does not match SDK common.schema.json#RequestEnvelope")
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
        .map_err(|_error| {
            ProviderFailure::invalid_request(
                request.request_id.clone(),
                "invalid_params",
                format!("{subcommand} request does not match its SDK provider/v1 operation schema"),
            )
        })
}
