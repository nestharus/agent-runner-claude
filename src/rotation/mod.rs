// declared_role: orchestration, formatter
// intrinsic_surface_declarations:
//   - component: src/rotation/mod.rs
//     role: intrinsic-surface
//     Domain: rotation_capability_module_index
//     Owns:
//       - rotation capability submodule declaration set
//       - rotation subcommand to handler routing surface

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod artifacts;
pub mod assess;
pub mod host_plan;
pub mod materialize;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "rotation.assess" => assess::handle(request),
        "rotation.materialize" => materialize::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_rotation_subcommand",
        format!("unsupported rotation subcommand: {subcommand}"),
    )
}
