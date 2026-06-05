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
        let mut flags = Vec::new();
        if !self.allowed_tools.is_empty() {
            flags.push("--allowed-tools".to_string());
            flags.push(tool_list(&self.allowed_tools));
        }
        if !self.disallowed_tools.is_empty() {
            flags.push("--disallowed-tools".to_string());
            flags.push(tool_list(&self.disallowed_tools));
        }
        if self.disable_slash_commands {
            flags.push("--disable-slash-commands".to_string());
        }
        flags
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
