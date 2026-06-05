// declared_role: predicate, accessor

pub fn is_session_marker(_text: &str) -> bool {
    false
}

pub fn initial_marker_name() -> &'static str {
    "launch_started"
}
