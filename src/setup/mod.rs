// declared_role: orchestration, formatter
// intrinsic_surface_declarations:
//   - component: src/setup/mod.rs
//     role: intrinsic-surface
//     Domain: setup_capability_module_index
//     Owns:
//       - setup capability submodule declaration set
//       - setup subcommand to handler routing surface

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod brain_cli;
pub mod brain_turn;
pub mod detect;
pub mod install_plan;
pub mod sync_plan;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "setup.detect" => detect::handle(request),
        "setup.install_plan" => install_plan::handle(request),
        "setup.sync_plan" => sync_plan::handle(request),
        "setup_brain.turn" => brain_turn::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_setup_subcommand",
        format!("unsupported setup subcommand: {subcommand}"),
    )
}
