use std::io::Read;

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) == Some(agent_runner_claude::NATIVE_EFFECT_GATE_ARG) {
        std::process::exit(agent_runner_claude::run_native_effect_gate(&args));
    }

    if args.get(1).map(String::as_str) == Some(agent_provider_execution::tool_bridge::SUBCOMMAND) {
        std::process::exit(agent_provider_execution::tool_bridge::main());
    }

    if args.get(1).map(String::as_str) == Some(agent_runner_claude::resident::SERVE) {
        std::process::exit(agent_runner_claude::resident::serve(&args));
    }

    let mut stdin = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("failed to read stdin: {err}");
        std::process::exit(2);
    }

    let exit_code = if args.get(1).map(String::as_str) == Some("launch") {
        // Launch events go through a delivery bound so a stalled host cannot
        // strand native custody.
        match agent_provider_execution::delivery::BoundedOutput::stdout() {
            Ok(mut output) => agent_runner_claude::write_invocation(&args, &stdin, &mut output),
            Err(_) => 1,
        }
    } else {
        agent_runner_claude::write_invocation(&args, &stdin, &mut std::io::stdout())
    };
    std::process::exit(exit_code);
}
