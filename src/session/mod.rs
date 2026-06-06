// declared_role: orchestration, formatter

use serde_json::Value;

use crate::envelope::decode::RequestEnvelope;
use crate::envelope::error::ProviderFailure;

pub mod atomic;
pub mod canonical;
pub mod capture;
pub mod export;
pub mod locate;
pub mod native_claude;
pub mod read_turns;
pub mod replace;
pub mod storage;
pub mod types;

pub fn handle(subcommand: &str, request: &RequestEnvelope) -> Result<Value, ProviderFailure> {
    match subcommand {
        "session.locate_transcript" => locate::handle(request),
        "session.read_turns" => read_turns::handle(request),
        "session.capture" => capture::handle(request),
        "session.export" => export::handle(request),
        "session.replace" => replace::handle(request),
        _ => Err(unsupported_subcommand(subcommand)),
    }
}

fn unsupported_subcommand(subcommand: &str) -> ProviderFailure {
    ProviderFailure::unsupported(
        "unknown_session_subcommand",
        format!("unsupported session subcommand: {subcommand}"),
    )
}
