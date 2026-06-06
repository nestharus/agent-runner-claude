// declared_role: accessor
// adapter_declarations:
//   - component: src/policy/params.rs
//     role: adapter
//     Translates:
//       - contract/v1/policy.schema.json#/$defs/PolicyEvaluateRequest
//       - contract/v1/policy.schema.json#/$defs/PolicyEvaluateParams

use serde_json::Value;

pub fn params_value(value: &Value) -> &Value {
    value
}
