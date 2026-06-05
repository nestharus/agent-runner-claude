// declared_role: formatter, mapper, orchestration
// facade_declarations:
//   - component: src/dispatch/router.rs
//     role: common-interface-facade
//     Owns:
//       - provider subcommand to capability handler routing table
//       - unsupported subcommand provider failure surface

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub fn route_request(
    subcommand: &str,
    request: &RequestEnvelope,
) -> Result<Value, ProviderFailure> {
    match subcommand {
        "describe" => crate::describe::handle(request),
        "schema" => crate::schema_lookup::handle(request),
        "settings.list" | "settings.get" | "settings.create" | "settings.update"
        | "settings.delete" | "settings.validate" | "settings.migrate" => {
            crate::settings::handle(subcommand, request)
        }
        "policy.evaluate" => crate::policy::handle(subcommand, request),
        "launch" => crate::launch::handle(subcommand, request),
        "terminal.classify" => crate::terminal::handle(subcommand, request),
        "quota.source" | "quota.probe" | "quota.refresh_auth" => {
            crate::quota::handle(subcommand, request)
        }
        "session.locate_transcript"
        | "session.read_turns"
        | "session.capture"
        | "session.export"
        | "session.replace" => crate::session::handle(subcommand, request),
        "rotation.assess" | "rotation.materialize" => crate::rotation::handle(subcommand, request),
        "discovery.models" | "discovery.accounts" => crate::discovery::handle(subcommand, request),
        "setup.detect" | "setup.install_plan" | "setup.sync_plan" | "setup_brain.turn" => {
            crate::setup::handle(subcommand, request)
        }
        "migration.plan" | "migration.apply" => crate::migration::handle(subcommand, request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_subcommand",
        format!("unsupported provider subcommand: {subcommand}"),
    )
}
