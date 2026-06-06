// declared_role: orchestration
// adapter_declarations:
//   - component: src/envelope/
//     role: adapter
//     Translates:
//       - contract/v1/common.schema.json#/$defs/RequestEnvelope
//       - contract/v1/common.schema.json#/$defs/SuccessResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorObject

pub mod decode;
pub mod error;
pub mod response;

pub const CONTRACT: &str = "oulipoly.provider/v1";
