// declared_role: formatter, mapper, orchestration, parser, predicate

use serde_json::Value;

#[derive(Debug, Default)]
pub struct ClaudeRestrictions {
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    pub disable_slash_commands: bool,
}

impl ClaudeRestrictions {
    pub fn append_argv(&self, argv: &mut Vec<String>) {
        argv.extend(self.argv_flags());
    }

    pub fn has_tool_conflict(&self) -> bool {
        !self.allowed_tools.is_empty() && !self.disallowed_tools.is_empty()
    }

    fn argv_flags(&self) -> Vec<String> {
        active_restriction_args(self)
            .into_iter()
            .flat_map(format_restriction_arg)
            .collect()
    }
}

enum RestrictionArg<'a> {
    AllowedTools(&'a [String]),
    DisallowedTools(&'a [String]),
    DisableSlashCommands,
}

fn active_restriction_args(restrictions: &ClaudeRestrictions) -> Vec<RestrictionArg<'_>> {
    let mut args = Vec::new();
    if has_allowed_tools(restrictions) {
        args.push(RestrictionArg::AllowedTools(&restrictions.allowed_tools));
    }
    if has_disallowed_tools(restrictions) {
        args.push(RestrictionArg::DisallowedTools(
            &restrictions.disallowed_tools,
        ));
    }
    if has_disable_slash_commands(restrictions) {
        args.push(RestrictionArg::DisableSlashCommands);
    }
    args
}

fn has_allowed_tools(restrictions: &ClaudeRestrictions) -> bool {
    !restrictions.allowed_tools.is_empty()
}

fn has_disallowed_tools(restrictions: &ClaudeRestrictions) -> bool {
    !restrictions.disallowed_tools.is_empty()
}

fn has_disable_slash_commands(restrictions: &ClaudeRestrictions) -> bool {
    restrictions.disable_slash_commands
}

fn format_restriction_arg(arg: RestrictionArg<'_>) -> Vec<String> {
    match arg {
        RestrictionArg::AllowedTools(tools) => {
            vec!["--allowed-tools".to_string(), tool_list(tools)]
        }
        RestrictionArg::DisallowedTools(tools) => {
            vec!["--disallowed-tools".to_string(), tool_list(tools)]
        }
        RestrictionArg::DisableSlashCommands => vec!["--disable-slash-commands".to_string()],
    }
}

pub fn parse(launch: &Value) -> Option<ClaudeRestrictions> {
    restriction_fields(launch).map(restrictions_from_fields)
}

fn restriction_fields(launch: &Value) -> Option<ClaudeRestrictionFields> {
    let Some(value) = launch.get("tool_restrictions") else {
        return Some(ClaudeRestrictionFields::default());
    };
    let object = value.as_object()?;
    if object.get("kind").and_then(Value::as_str) != Some("claude") {
        return Some(ClaudeRestrictionFields::default());
    }
    let claude = object.get("claude")?.as_object()?;
    Some(ClaudeRestrictionFields {
        allowed_tools: string_array(claude.get("allowed_tools"))?,
        disallowed_tools: string_array(claude.get("disallowed_tools"))?,
        disable_slash_commands: claude
            .get("disable_slash_commands")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[derive(Debug, Default)]
struct ClaudeRestrictionFields {
    allowed_tools: Vec<String>,
    disallowed_tools: Vec<String>,
    disable_slash_commands: bool,
}

fn restrictions_from_fields(fields: ClaudeRestrictionFields) -> ClaudeRestrictions {
    ClaudeRestrictions {
        allowed_tools: fields.allowed_tools,
        disallowed_tools: fields.disallowed_tools,
        disable_slash_commands: fields.disable_slash_commands,
    }
}

fn string_array(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        None => Some(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect(),
        Some(_) => None,
    }
}

fn tool_list(tools: &[String]) -> String {
    tools.join(",")
}
