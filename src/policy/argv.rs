// declared_role: accessor, mapper, orchestration, parser

use serde_json::Value;

pub fn base_argv(launch: &Value, provider_args: &[String]) -> Option<Vec<String>> {
    let command = command_tokens(launch)?;
    let launch_args = launch_args(launch)?;
    Some(assemble_argv(command, provider_args, launch_args))
}

fn command_tokens(launch: &Value) -> Option<Vec<String>> {
    let command = launch.get("command")?.as_str()?;
    Some(split_command(command))
}

fn launch_args(launch: &Value) -> Option<Vec<String>> {
    let Some(args) = launch.get("args") else {
        return Some(Vec::new());
    };
    args.as_array()?
        .iter()
        .map(|arg| arg.as_str().map(str::to_string))
        .collect()
}

fn assemble_argv(
    mut command: Vec<String>,
    provider_args: &[String],
    launch_args: Vec<String>,
) -> Vec<String> {
    command.extend(provider_args.iter().cloned());
    command.extend(launch_args);
    command
}

fn split_command(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}
