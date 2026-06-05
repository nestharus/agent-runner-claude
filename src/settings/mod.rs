// declared_role: orchestration, formatter

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod create;
pub mod delete;
pub mod get;
pub mod list;
pub mod lock;
pub mod migrate;
pub mod store;
pub mod summary;
pub mod update;
pub mod validate;
pub mod version;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "settings.list" => list::handle(request),
        "settings.get" => get::handle(request),
        "settings.create" => create::handle(request),
        "settings.update" => update::handle(request),
        "settings.delete" => delete::handle(request),
        "settings.validate" => validate::handle(request),
        "settings.migrate" => migrate::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_settings_subcommand",
        format!("unsupported settings subcommand: {subcommand}"),
    )
}
