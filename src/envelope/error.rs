// declared_role: accessor, mapper, formatter, orchestration
// adapter_declarations:
//   - component: src/envelope/
//     role: adapter
//     Translates:
//       - contract/v1/common.schema.json#/$defs/RequestEnvelope
//       - contract/v1/common.schema.json#/$defs/SuccessResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorResponseEnvelope
//       - contract/v1/common.schema.json#/$defs/ErrorObject

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Unsupported,
    InvalidRequest,
    InvalidSettings,
    Unavailable,
    Timeout,
    Conflict,
    Failed,
}

impl ErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::InvalidRequest => "invalid_request",
            Self::InvalidSettings => "invalid_settings",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::Conflict => "conflict",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderFailure {
    pub request_id: Option<Box<str>>,
    pub code: Box<str>,
    pub category: ErrorCategory,
    pub message: Box<str>,
    pub retryable: bool,
    pub details: Option<Box<Value>>,
    pub diagnostics: Option<Vec<Value>>,
}

impl ProviderFailure {
    pub fn new(
        category: ErrorCategory,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            request_id: None,
            code: code.into().into_boxed_str(),
            category,
            message: message.into().into_boxed_str(),
            retryable,
            details: None,
            diagnostics: None,
        }
    }

    pub fn invalid_request(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::InvalidRequest, code, message, false)
    }

    pub fn invalid_request_with_request_id(
        request_id: String,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::invalid_request(code, message).with_request_id(request_id)
    }

    pub fn unsupported(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Unsupported, code, message, false)
    }

    pub fn not_implemented(capability: &str) -> Self {
        Self::new(
            ErrorCategory::Failed,
            "capability_not_implemented",
            not_implemented_message(capability),
            false,
        )
    }

    pub fn with_request_id(mut self, request_id: String) -> Self {
        self.request_id = Some(request_id.into_boxed_str());
        self
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(Box::new(details));
        self
    }

    pub fn request_id(&self) -> &str {
        self.request_id.as_deref().unwrap_or("unknown-request")
    }

    pub fn exit_code(&self) -> u8 {
        match self.category {
            ErrorCategory::Failed | ErrorCategory::Conflict => 1,
            ErrorCategory::InvalidRequest | ErrorCategory::InvalidSettings => 2,
            ErrorCategory::Unsupported => 3,
            ErrorCategory::Unavailable => 4,
            ErrorCategory::Timeout => 5,
        }
    }
}

fn not_implemented_message(capability: &str) -> String {
    format!("{capability} is routed but not implemented in this foundation pass")
}
