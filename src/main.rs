// declared_role: orchestration

fn main() -> std::process::ExitCode {
    agent_runner_claude::run_cli(
        std::env::args(),
        std::io::stdin(),
        std::io::stdout(),
        std::io::stderr(),
    )
}
