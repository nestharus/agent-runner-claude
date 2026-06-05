// declared_role: filter, formatter, orchestration, parser, predicate

pub fn evidence(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    non_empty_evidence(&evidence_text(stdout, stderr)).map(|text| bound_evidence(text, 200))
}

fn evidence_text(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::new();
    if !stdout.is_empty() {
        text.push_str(&String::from_utf8_lossy(stdout));
    }
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(stderr));
    }
    text
}

fn non_empty_evidence(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty()).then_some(text)
}

pub fn bound_evidence(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
