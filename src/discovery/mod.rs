// declared_role: orchestration, formatter

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod accounts;
pub mod models;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "discovery.models" => models::handle(request),
        "discovery.accounts" => accounts::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_discovery_subcommand",
        format!("unsupported discovery subcommand: {subcommand}"),
    )
}
