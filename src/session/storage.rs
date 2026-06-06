// declared_role: accessor, filter, formatter, mapper, orchestration, parser, predicate, validator
// intrinsic_surface_declarations:
//   - component: src/session/storage.rs
//     role: intrinsic-surface
//     Domain: claude_code_transcript_storage
//     Owns:
//       - "~/.claude/projects/** layout"
//       - "filename and content-fallback session lookup"
//       - "native transcript file lifecycle"
//       - "src/fs/paths.rs transcript-root confinement and HOME expansion seam"
//       - "src/session/native_claude.rs text_contains_session content-match helper"

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::native_claude;

#[derive(Debug, Clone, Copy)]
pub struct ScanBounds {
    pub max_depth: usize,
    pub max_entries: usize,
}

impl Default for ScanBounds {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_entries: 4096,
        }
    }
}

#[derive(Debug, Clone)]
pub enum LocateOutcome {
    Missing,
    Found(PathBuf),
    Ambiguous(Vec<PathBuf>),
}

#[derive(Debug)]
pub enum TranscriptPathError {
    RootUnavailable(io::Error),
    TargetUnavailable(io::Error),
    OutsideProviderRoot { root: PathBuf, target: PathBuf },
    NonTranscriptFile(PathBuf),
}

impl std::fmt::Display for TranscriptPathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootUnavailable(error) => write!(
                formatter,
                "Claude transcript storage root is unavailable: {error}"
            ),
            Self::TargetUnavailable(error) => {
                write!(formatter, "transcript target cannot be resolved: {error}")
            }
            Self::OutsideProviderRoot { root, target } => write!(
                formatter,
                "transcript target {} is outside Claude transcript storage root {}",
                target.display(),
                root.display()
            ),
            Self::NonTranscriptFile(path) => write!(
                formatter,
                "transcript target {} is not a Claude JSONL transcript",
                path.display()
            ),
        }
    }
}

pub fn claude_projects_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}

pub fn scan_bounds(params: &Value) -> ScanBounds {
    bounds_from_scan(scan_params(params))
}

pub fn confined_transcript_path(
    home: &Path,
    raw_path: &str,
) -> Result<PathBuf, TranscriptPathError> {
    let root = claude_projects_dir(home);
    let target = resolved_target_path(home, raw_path);
    let target = confined_transcript_target(&root, &target)?;
    require_jsonl_transcript(&target)?;
    Ok(target)
}

fn scan_params(params: &Value) -> Option<&Value> {
    params.get("scan")
}

fn bounds_from_scan(scan: Option<&Value>) -> ScanBounds {
    let mut bounds = ScanBounds::default();
    if let Some(scan) = scan {
        if let Some(max_depth) = scan.get("max_depth").and_then(Value::as_u64) {
            bounds.max_depth = max_depth as usize;
        }
        if let Some(max_entries) = scan.get("max_entries").and_then(Value::as_u64) {
            bounds.max_entries = max_entries as usize;
        }
    }
    bounds
}

pub fn locate_by_session_id(
    home: &Path,
    session_id: &str,
    bounds: ScanBounds,
) -> io::Result<LocateOutcome> {
    let projects = claude_projects_dir(home);
    if !projects_exist(&projects) {
        return Ok(LocateOutcome::Missing);
    }
    locate_outcome(select_matches(scan_matches(&projects, session_id, bounds)?))
}

pub fn read_transcript(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    bounds: ScanBounds,
    visited: &mut usize,
    on_file: &mut impl FnMut(&Path),
) -> io::Result<()> {
    if !validate_scan_bounds(depth, *visited, bounds) {
        return Ok(());
    }

    let entries = sorted_scan_entry_paths(dir)?;
    scan_entries(root, entries, depth, bounds, visited, on_file)
}

fn scan_entries(
    root: &Path,
    entries: Vec<PathBuf>,
    depth: usize,
    bounds: ScanBounds,
    visited: &mut usize,
    on_file: &mut impl FnMut(&Path),
) -> io::Result<()> {
    for path in entries {
        if !validate_scan_entry_count(*visited, bounds) {
            break;
        }
        count_scan_entry(visited);
        scan_entry(root, path, depth, bounds, visited, on_file)?;
    }
    Ok(())
}

fn sorted_scan_entry_paths(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut entries = scan_entry_paths(dir)?;
    sort_paths(&mut entries);
    Ok(entries)
}

fn count_scan_entry(visited: &mut usize) {
    *visited += 1;
}

fn scan_entry(
    root: &Path,
    path: PathBuf,
    depth: usize,
    bounds: ScanBounds,
    visited: &mut usize,
    on_file: &mut impl FnMut(&Path),
) -> io::Result<()> {
    let confined = confined_scan_path(root, &path)?;
    apply_scan_entry_action(
        scan_entry_action(confined),
        root,
        depth,
        bounds,
        visited,
        on_file,
    )
}

enum ScanEntryAction {
    Recurse(PathBuf),
    Visit(PathBuf),
    Skip,
}

fn scan_entry_action(path: PathBuf) -> ScanEntryAction {
    if is_scan_directory(&path) {
        ScanEntryAction::Recurse(path)
    } else if is_scan_transcript_file(&path) {
        ScanEntryAction::Visit(path)
    } else {
        ScanEntryAction::Skip
    }
}

fn apply_scan_entry_action(
    action: ScanEntryAction,
    root: &Path,
    depth: usize,
    bounds: ScanBounds,
    visited: &mut usize,
    on_file: &mut impl FnMut(&Path),
) -> io::Result<()> {
    match action {
        ScanEntryAction::Recurse(path) => {
            scan_dir(root, &path, depth + 1, bounds, visited, on_file)
        }
        ScanEntryAction::Visit(path) => {
            on_file(&path);
            Ok(())
        }
        ScanEntryAction::Skip => Ok(()),
    }
}

fn validate_scan_bounds(depth: usize, visited: usize, bounds: ScanBounds) -> bool {
    depth <= bounds.max_depth && validate_scan_entry_count(visited, bounds)
}

fn validate_scan_entry_count(visited: usize, bounds: ScanBounds) -> bool {
    visited < bounds.max_entries
}

fn scan_entry_paths(dir: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(entries) => Ok(entry_paths(successful_entries(entries))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn successful_entries(entries: fs::ReadDir) -> Vec<fs::DirEntry> {
    entries.filter_map(Result::ok).collect()
}

fn entry_paths(entries: Vec<fs::DirEntry>) -> Vec<PathBuf> {
    entries.into_iter().map(entry_path).collect()
}

fn entry_path(entry: fs::DirEntry) -> PathBuf {
    entry.path()
}

fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort();
}

fn is_scan_directory(path: &Path) -> bool {
    path.is_dir()
}

fn is_scan_transcript_file(path: &Path) -> bool {
    is_jsonl_path(path)
}

fn is_jsonl_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
}

fn projects_exist(projects: &Path) -> bool {
    projects.exists()
}

#[derive(Debug, Default)]
struct TranscriptMatches {
    filename_matches: Vec<PathBuf>,
    content_matches: Vec<PathBuf>,
}

#[derive(Debug)]
struct TranscriptCandidate {
    path: PathBuf,
    text: Option<String>,
}

fn scan_matches(
    projects: &Path,
    session_id: &str,
    bounds: ScanBounds,
) -> io::Result<TranscriptMatches> {
    let paths = scan_transcript_paths(projects, bounds)?;
    let candidates = read_transcript_candidates(paths);
    Ok(transcript_matches(&candidates, session_id))
}

fn scan_transcript_paths(projects: &Path, bounds: ScanBounds) -> io::Result<Vec<PathBuf>> {
    let mut visited = 0usize;
    let mut paths = Vec::new();
    scan_dir(projects, projects, 0, bounds, &mut visited, &mut |path| {
        paths.push(path.to_path_buf());
    })?;
    Ok(paths)
}

fn read_transcript_candidates(paths: Vec<PathBuf>) -> Vec<TranscriptCandidate> {
    paths.into_iter().map(transcript_candidate).collect()
}

fn transcript_candidate(path: PathBuf) -> TranscriptCandidate {
    let text = transcript_candidate_text(&path);
    transcript_candidate_value(path, text)
}

fn transcript_candidate_value(path: PathBuf, text: Option<String>) -> TranscriptCandidate {
    TranscriptCandidate { path, text }
}

fn transcript_candidate_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn transcript_matches(candidates: &[TranscriptCandidate], session_id: &str) -> TranscriptMatches {
    let mut matches = TranscriptMatches::default();
    for candidate in candidates {
        collect_match(candidate, session_id, &mut matches);
    }
    matches
}

fn collect_match(
    candidate: &TranscriptCandidate,
    session_id: &str,
    matches: &mut TranscriptMatches,
) {
    record_candidate_match(
        matches,
        candidate_path(candidate),
        match_classification(candidate, session_id),
    );
}

struct MatchClassification {
    filename: bool,
    content: bool,
}

fn match_classification(candidate: &TranscriptCandidate, session_id: &str) -> MatchClassification {
    MatchClassification {
        filename: file_name_matches(candidate_path(candidate), session_id),
        content: candidate_text_matches(candidate, session_id),
    }
}

fn candidate_path(candidate: &TranscriptCandidate) -> &Path {
    &candidate.path
}

fn candidate_text(candidate: &TranscriptCandidate) -> Option<&str> {
    candidate.text.as_deref()
}

fn candidate_text_matches(candidate: &TranscriptCandidate, session_id: &str) -> bool {
    candidate_text(candidate)
        .is_some_and(|text| native_claude::text_contains_session(text, session_id))
}

fn record_candidate_match(
    matches: &mut TranscriptMatches,
    path: &Path,
    classification: MatchClassification,
) {
    if filename_match(&classification) {
        record_filename_match(matches, path);
    }
    if content_match(&classification) {
        record_content_match(matches, path);
    }
}

fn filename_match(classification: &MatchClassification) -> bool {
    classification.filename
}

fn content_match(classification: &MatchClassification) -> bool {
    classification.content
}

fn record_filename_match(matches: &mut TranscriptMatches, path: &Path) {
    matches.filename_matches.push(path.to_path_buf());
}

fn record_content_match(matches: &mut TranscriptMatches, path: &Path) {
    matches.content_matches.push(path.to_path_buf());
}

fn select_matches(matches: TranscriptMatches) -> Vec<PathBuf> {
    sorted_unique(preferred_matches(matches))
}

fn preferred_matches(matches: TranscriptMatches) -> Vec<PathBuf> {
    if matches.content_matches.is_empty() {
        matches.filename_matches
    } else {
        matches.content_matches
    }
}

fn sorted_unique(mut matches: Vec<PathBuf>) -> Vec<PathBuf> {
    matches.sort();
    matches.dedup();
    matches
}

fn locate_outcome(mut matches: Vec<PathBuf>) -> io::Result<LocateOutcome> {
    match matches.len() {
        0 => Ok(LocateOutcome::Missing),
        1 => Ok(LocateOutcome::Found(matches.remove(0))),
        _ => Ok(LocateOutcome::Ambiguous(matches)),
    }
}

fn file_name_matches(path: &Path, session_id: &str) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem == session_id)
}

fn resolved_target_path(home: &Path, raw_path: &str) -> PathBuf {
    crate::fs::paths::expand_home(raw_path, home.to_str())
}

fn confined_transcript_target(root: &Path, target: &Path) -> Result<PathBuf, TranscriptPathError> {
    crate::fs::paths::confined_child_path(root, target).map_err(path_error)
}

fn confined_scan_path(root: &Path, target: &Path) -> io::Result<PathBuf> {
    crate::fs::paths::confined_child_path(root, target).map_err(scan_path_permission_denied)
}

fn scan_path_permission_denied(error: crate::fs::paths::PathConfinementError) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("scan-discovered transcript path is outside provider root: {error}"),
    )
}

fn require_jsonl_transcript(path: &Path) -> Result<(), TranscriptPathError> {
    if is_jsonl_path(path) {
        Ok(())
    } else {
        Err(non_transcript_file(path))
    }
}

fn non_transcript_file(path: &Path) -> TranscriptPathError {
    TranscriptPathError::NonTranscriptFile(path.to_path_buf())
}

fn path_error(error: crate::fs::paths::PathConfinementError) -> TranscriptPathError {
    match error {
        crate::fs::paths::PathConfinementError::RootUnavailable(error) => {
            TranscriptPathError::RootUnavailable(error)
        }
        crate::fs::paths::PathConfinementError::TargetUnavailable(error) => {
            TranscriptPathError::TargetUnavailable(error)
        }
        crate::fs::paths::PathConfinementError::OutsideRoot { root, target } => {
            TranscriptPathError::OutsideProviderRoot { root, target }
        }
    }
}
