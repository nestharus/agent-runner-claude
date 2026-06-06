// declared_role: mapper, predicate
// exit-code/status-only per AGE-166

use crate::terminal::status::ProcessStatus;

pub fn classify(status: ProcessStatus, _stdout: &[u8], _stderr: &[u8]) -> &'static str {
    match status {
        ProcessStatus::Exited(0) => "clean_exit",
        ProcessStatus::SpawnError => "spawn_error",
        ProcessStatus::Cancelled => "cancelled",
        ProcessStatus::ProlongedSilence => "prolonged_silence",
        ProcessStatus::SignalTerminated(_) => "signal_exit",
        ProcessStatus::Unknown => "unknown",
        ProcessStatus::Exited(_) => "nonzero_exit",
    }
}
