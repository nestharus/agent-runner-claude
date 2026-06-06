// declared_role: orchestration, formatter
// intrinsic_surface_declarations:
//   - component: src/migration/mod.rs
//     role: intrinsic-surface
//     Domain: migration_capability_module_index
//     Owns:
//       - migration capability submodule declaration set
//       - migration subcommand to handler routing surface

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
