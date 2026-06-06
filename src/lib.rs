// declared_role: orchestration, accessor, formatter
// facade_declarations:
//   - component: src/lib.rs
//     role: common-interface-facade
//     Owns:
//       - crate module declaration set for provider capability families
//       - public provider CLI run surface

use std::io::{Read, Write};
use std::process::ExitCode;

pub mod describe;
pub mod discovery;
pub mod dispatch;
pub mod encoding;
pub mod envelope;
pub mod external;
pub mod fs;
pub mod launch;
pub mod migration;
pub mod policy;
pub mod quota;
pub mod rotation;
pub mod schema_lookup;
pub mod session;
pub mod settings;
pub mod settings_schema;
pub mod setup;
pub mod terminal;

use envelope::error::ProviderFailure;

pub fn run_cli<I, R, W, E>(
    args: I,
    stdin_reader: R,
    mut stdout_writer: W,
    mut stderr_writer: E,
) -> ExitCode
where
    I: IntoIterator,
    I::Item: Into<String>,
    R: Read,
    W: Write,
    E: Write,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let launch_requested = is_launch_requested(&args);

    let request = match envelope::decode::decode_request(stdin_reader) {
        Ok(request) => request,
        Err(failure) if launch_requested => {
            launch::stream_rejected_request_and_exit(failure.request_id(), &failure.message)
        }
        Err(failure) => return write_failure(&mut stdout_writer, &mut stderr_writer, failure),
    };

    let subcommand = match dispatch::subcommand::subcommand_from_args(args) {
        Ok(subcommand) => subcommand,
        Err(failure) => {
            return write_failure_with_request(
                &mut stdout_writer,
                &mut stderr_writer,
                &request.request_id,
                &failure,
            );
        }
    };

    match dispatch::router::route_request(&subcommand, &request) {
        Ok(result) => write_success(
            &mut stdout_writer,
            &mut stderr_writer,
            &request.request_id,
            result,
        ),
        Err(failure) => write_failure_with_request(
            &mut stdout_writer,
            &mut stderr_writer,
            &request.request_id,
            &failure,
        ),
    }
}

fn is_launch_requested(args: &[String]) -> bool {
    dispatch::subcommand::subcommand_from_args(args.to_vec())
        .is_ok_and(|subcommand| subcommand == "launch")
}

fn write_failure<W: Write, E: Write>(
    stdout_writer: &mut W,
    stderr_writer: &mut E,
    failure: ProviderFailure,
) -> ExitCode {
    let response = envelope::response::error_response(failure.request_id(), &failure);
    write_response(stdout_writer, stderr_writer, &response, failure.exit_code())
}

fn write_success<W: Write, E: Write>(
    stdout_writer: &mut W,
    stderr_writer: &mut E,
    request_id: &str,
    result: serde_json::Value,
) -> ExitCode {
    let response = envelope::response::success_response(request_id, result);
    write_response(stdout_writer, stderr_writer, &response, 0)
}

fn write_failure_with_request<W: Write, E: Write>(
    stdout_writer: &mut W,
    stderr_writer: &mut E,
    request_id: &str,
    failure: &ProviderFailure,
) -> ExitCode {
    let response = envelope::response::error_response(request_id, failure);
    write_response(stdout_writer, stderr_writer, &response, failure.exit_code())
}

fn write_response<W: Write, E: Write>(
    stdout_writer: &mut W,
    stderr_writer: &mut E,
    response: &serde_json::Value,
    exit_code: u8,
) -> ExitCode {
    match response_bytes(response).and_then(|bytes| write_stdout(stdout_writer, &bytes)) {
        Ok(()) => ExitCode::from(exit_code),
        Err(error) => write_protocol_failure(stderr_writer, &error),
    }
}

fn write_protocol_failure<E: Write>(stderr_writer: &mut E, error: &serde_json::Error) -> ExitCode {
    let _ = writeln!(stderr_writer, "{}", protocol_write_failure_message(error));
    protocol_write_failure_exit_code()
}

fn protocol_write_failure_message(error: &serde_json::Error) -> String {
    format!("provider protocol write failure: {error}")
}

fn protocol_write_failure_exit_code() -> ExitCode {
    ExitCode::from(64)
}

fn response_bytes(response: &serde_json::Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_stdout<W: Write>(stdout_writer: &mut W, bytes: &[u8]) -> Result<(), serde_json::Error> {
    stdout_writer
        .write_all(bytes)
        .map_err(serde_json::Error::io)?;
    stdout_writer.flush().map_err(serde_json::Error::io)
}
