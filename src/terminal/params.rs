// declared_role: accessor
// adapter_declarations:
//   - component: src/terminal/params.rs
//     role: adapter
//     Translates:
//       - contract/v1/terminal.schema.json#/$defs/TerminalClassifyRequest
//       - contract/v1/terminal.schema.json#/$defs/TerminalClassifyParams
//       - contract/v1/common.schema.json#/$defs/ProcessStatus

use serde_json::Value;

pub fn params_value(value: &Value) -> &Value {
    value
}
