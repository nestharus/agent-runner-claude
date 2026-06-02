use std::io::Read;

fn main() {
    let mut stdin = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut stdin) {
        eprintln!("failed to read stdin: {err}");
        std::process::exit(2);
    }

    let args = std::env::args().collect::<Vec<_>>();
    let output = agent_runner_claude::handle_invocation(&args, &stdin);
    print!("{}", output.stdout);
    std::process::exit(output.exit_code);
}
