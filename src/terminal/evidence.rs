// declared_role: filter, formatter, orchestration, parser, predicate

pub fn evidence(stdout: &[u8], stderr: &[u8]) -> Option<String> {
    bounded_selected_evidence(non_empty_evidence(&evidence_text(stdout, stderr)))
}

fn bounded_selected_evidence(text: Option<&str>) -> Option<String> {
    text.map(format_bounded_evidence)
}

fn format_bounded_evidence(text: &str) -> String {
    bound_evidence(text, 200)
}

fn evidence_text(stdout: &[u8], stderr: &[u8]) -> String {
    format_evidence_fragments(evidence_fragments(stdout, stderr))
}

fn evidence_fragments(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
    evidence_streams(stdout, stderr)
        .into_iter()
        .map(lossy_stream_text)
        .collect()
}

fn evidence_streams<'a>(stdout: &'a [u8], stderr: &'a [u8]) -> Vec<&'a [u8]> {
    [stdout, stderr]
        .into_iter()
        .filter_map(non_empty_stream)
        .collect()
}

fn non_empty_stream(bytes: &[u8]) -> Option<&[u8]> {
    (!bytes.is_empty()).then_some(bytes)
}

fn lossy_stream_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

fn format_evidence_fragments(fragments: Vec<String>) -> String {
    fragments.join("\n")
}

fn non_empty_evidence(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty()).then_some(text)
}

pub fn bound_evidence(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
