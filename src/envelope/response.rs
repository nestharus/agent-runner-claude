// declared_role: accessor, formatter, mapper
// adapter_declarations:
//   - component: src/envelope/
//     role: adapter
//     Translates:
//       - contract/v1/common.schema.json#/$defs/RequestEnvelope
//       - contract/v1/common.schema.json#/$defs/SuccessResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorObject

use serde_json::{json, Map, Value};

use super::error::ProviderFailure;
use super::CONTRACT;

pub fn success_response(request_id: &str, result: Value) -> Value {
    json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "ok": true,
        "result": result,
    })
}

pub fn error_response(request_id: &str, failure: &ProviderFailure) -> Value {
    let mut error = Map::new();
    error.insert("code".to_string(), json!(&*failure.code));
    error.insert("category".to_string(), json!(failure.category.as_str()));
    error.insert("message".to_string(), json!(&*failure.message));
    error.insert("retryable".to_string(), json!(failure.retryable));
    if let Some(details) = &failure.details {
        error.insert("details".to_string(), (**details).clone());
    }
    if let Some(diagnostics) = &failure.diagnostics {
        error.insert("diagnostics".to_string(), Value::Array(diagnostics.clone()));
    }

    json!({
        "contract": CONTRACT,
        "request_id": request_id,
        "ok": false,
        "error": Value::Object(error),
    })
}
