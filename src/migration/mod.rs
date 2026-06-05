// declared_role: orchestration, formatter

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod apply;
pub mod legacy;
pub mod plan;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "migration.plan" => plan::handle(request),
        "migration.apply" => apply::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_migration_subcommand",
        format!("unsupported migration subcommand: {subcommand}"),
    )
}
