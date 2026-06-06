// declared_role: orchestration, formatter
// intrinsic_surface_declarations:
//   - component: src/quota/mod.rs
//     role: intrinsic-surface
//     Domain: quota_capability_module_index
//     Owns:
//       - quota capability submodule declaration set
//       - quota subcommand to handler routing surface

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod parse;
pub mod probe;
pub mod refresh_auth;
pub mod scripts;
pub mod source;
pub mod window;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "quota.source" => source::handle(request),
        "quota.probe" => probe::handle(request),
        "quota.refresh_auth" => refresh_auth::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_quota_subcommand",
        format!("unsupported quota subcommand: {subcommand}"),
    )
}
