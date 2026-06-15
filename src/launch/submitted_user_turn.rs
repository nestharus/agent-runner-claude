// declared_role: accessor, formatter, mapper, parser, predicate

use serde_json::{json, Value};

const SUBMITTED_USER_TURN_MARKER: &str = "oulipoly.submitted_user_turn";
const SUBMITTED_USER_TURN_SOURCE: &str = "claudecode.launch";
const DELIVERY_NONCE_PREFIX: &str = "[OULIPOLY-DELIVERY ";
const DELIVERY_NONCE_SUFFIX: char = ']';

#[derive(Clone)]
pub struct ResumeConfirmation {
    session_id: String,
    prompt_sha256: String,
    delivery_nonce: Option<String>,
}

impl ResumeConfirmation {
    // declared_role: accessor
    pub fn session_id(&self) -> &str {
        self.session_id.as_str()
    }
}

struct SubmittedResumePayload<'a> {
    bytes: &'a [u8],
    text: Option<&'a str>,
}

// declared_role: mapper
pub fn resume_confirmation(
    params: &Value,
    argv: &[String],
    stdin: &[u8],
) -> Option<ResumeConfirmation> {
    let session_id = known_provider_session_id(params)?;
    let prompt = submitted_resume_payload(params, argv, stdin)?;
    let delivery_nonce = prompt.text.and_then(delivery_nonce_from_prompt);
    Some(ResumeConfirmation {
        session_id: session_id.to_string(),
        prompt_sha256: crate::encoding::sha256_hex(prompt.bytes),
        delivery_nonce,
    })
}

// declared_role: formatter
pub fn marker_name() -> &'static str {
    SUBMITTED_USER_TURN_MARKER
}

// declared_role: formatter
pub fn marker_value(confirmation: &ResumeConfirmation) -> Value {
    submitted_user_turn_marker(confirmation)
}

// declared_role: accessor
fn known_provider_session_id(params: &Value) -> Option<&str> {
    nonblank_optional_text(raw_known_provider_session_id(params))
}

// declared_role: accessor
fn raw_known_provider_session_id(params: &Value) -> Option<&str> {
    params
        .get("session")
        .and_then(|session| session.get("known_provider_session_id"))
        .and_then(Value::as_str)
}

// declared_role: mapper
fn submitted_resume_payload<'a>(
    params: &'a Value,
    argv: &'a [String],
    stdin: &'a [u8],
) -> Option<SubmittedResumePayload<'a>> {
    stdin_payload(stdin).or_else(|| prompt_arg_payload(argv, prompt_input(params)))
}

// declared_role: accessor
fn stdin_payload(bytes: &[u8]) -> Option<SubmittedResumePayload<'_>> {
    let bytes = nonempty_payload_bytes(bytes)?;
    Some(SubmittedResumePayload {
        bytes,
        text: payload_utf8_text(bytes),
    })
}

// declared_role: predicate
fn nonempty_payload_bytes(bytes: &[u8]) -> Option<&[u8]> {
    (!bytes_are_empty_payload(bytes)).then_some(bytes)
}

// declared_role: predicate
fn bytes_are_empty_payload(bytes: &[u8]) -> bool {
    payload_text_or_bytes_are_empty(bytes, payload_utf8_text(bytes))
}

// declared_role: predicate
fn payload_text_or_bytes_are_empty(bytes: &[u8], text: Option<&str>) -> bool {
    text.map_or_else(|| bytes.is_empty(), text_is_blank)
}

// declared_role: parser
fn payload_utf8_text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}

// declared_role: accessor
fn prompt_input(params: &Value) -> Option<&str> {
    params
        .get("model")
        .and_then(|model| model.get("inputs"))
        .and_then(|inputs| inputs.get("prompt"))
        .and_then(Value::as_str)
}

// declared_role: mapper
fn prompt_arg_payload<'a>(
    argv: &'a [String],
    prompt: Option<&'a str>,
) -> Option<SubmittedResumePayload<'a>> {
    let prompt = nonblank_optional_text(prompt)?;
    prompt_present_in_argv(argv, prompt).then(|| prompt_text_payload(prompt))
}

// declared_role: predicate
fn prompt_present_in_argv(argv: &[String], prompt: &str) -> bool {
    argv.iter().any(|arg| arg == prompt)
}

// declared_role: mapper
fn prompt_text_payload(prompt: &str) -> SubmittedResumePayload<'_> {
    SubmittedResumePayload {
        bytes: prompt.as_bytes(),
        text: Some(prompt),
    }
}

// declared_role: accessor
fn nonblank_optional_text(value: Option<&str>) -> Option<&str> {
    value.filter(|text| is_nonblank_text(text))
}

// declared_role: predicate
fn is_nonblank_text(text: &str) -> bool {
    !text_is_blank(text)
}

// declared_role: predicate
fn text_is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

// declared_role: parser
fn delivery_nonce_from_prompt(prompt: &str) -> Option<String> {
    let start = prompt.find(DELIVERY_NONCE_PREFIX)? + DELIVERY_NONCE_PREFIX.len();
    let tail = &prompt[start..];
    let end = tail.find(DELIVERY_NONCE_SUFFIX)?;
    let nonce = tail[..end].trim();
    (!nonce.is_empty()).then(|| nonce.to_string())
}

// declared_role: formatter
fn submitted_user_turn_marker(confirmation: &ResumeConfirmation) -> Value {
    let marker = submitted_user_turn_marker_base(confirmation);
    marker_with_delivery_nonce(marker, confirmation.delivery_nonce.as_deref())
}

// declared_role: formatter
fn submitted_user_turn_marker_base(confirmation: &ResumeConfirmation) -> Value {
    json!({
        "provider_session_id": confirmation.session_id.as_str(),
        "prompt_sha256": confirmation.prompt_sha256.as_str(),
        "source": SUBMITTED_USER_TURN_SOURCE,
    })
}

// declared_role: formatter
fn marker_with_delivery_nonce(mut marker: Value, delivery_nonce: Option<&str>) -> Value {
    if let Some(delivery_nonce) = delivery_nonce {
        marker["delivery_nonce"] = json!(delivery_nonce);
    }
    marker
}
