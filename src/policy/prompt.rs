// declared_role: accessor, mapper, predicate

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PromptProjection {
    pub prompt: Option<String>,
    pub stdin: Option<String>,
}

pub fn project(model: &Value, launch: &Value) -> Option<PromptProjection> {
    let prompt = model
        .get("inputs")?
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_string);
    match launch
        .get("prompt_mode")
        .and_then(Value::as_str)
        .unwrap_or("arg")
    {
        "stdin" => Some(PromptProjection {
            prompt: None,
            stdin: prompt,
        }),
        "arg" => Some(PromptProjection {
            prompt,
            stdin: None,
        }),
        _ => None,
    }
}
