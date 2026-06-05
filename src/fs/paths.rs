// declared_role: accessor, formatter, mapper, predicate, validator
// intrinsic_surface_declarations:
//   - component: src/fs/paths.rs
//     role: intrinsic-surface
//     Domain: provider_host_context_path_resolution_and_confinement
//     Owns:
//       - "host.data_root provider directory resolution"
//       - "HOME expansion for host-declared paths"
//       - "provider-root confinement and safe filename normalization"

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::envelope::error::ProviderFailure;

pub fn expand_home(path: &str, home: Option<&str>) -> PathBuf {
    if path == "~" {
        return PathBuf::from(home.unwrap_or("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return PathBuf::from(home.unwrap_or("~")).join(rest);
    }
    PathBuf::from(path)
}

pub fn provider_data_dir(host: &Value) -> Result<PathBuf, ProviderFailure> {
    Ok(host_data_root(host)?.join("claude"))
}

pub fn host_data_root(host: &Value) -> Result<PathBuf, ProviderFailure> {
    host.get("data_root")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            ProviderFailure::invalid_request(
                "missing_host_data_root",
                "host.data_root is required for provider data path resolution",
            )
        })
}

#[derive(Debug)]
pub enum PathConfinementError {
    RootUnavailable(io::Error),
    TargetUnavailable(io::Error),
    OutsideRoot { root: PathBuf, target: PathBuf },
}

impl std::fmt::Display for PathConfinementError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootUnavailable(error) => {
                write!(formatter, "provider root cannot be resolved: {error}")
            }
            Self::TargetUnavailable(error) => {
                write!(formatter, "candidate path cannot be resolved: {error}")
            }
            Self::OutsideRoot { root, target } => write!(
                formatter,
                "candidate path {} is outside provider root {}",
                target.display(),
                root.display()
            ),
        }
    }
}

pub fn confined_child_path(root: &Path, candidate: &Path) -> Result<PathBuf, PathConfinementError> {
    confined_path(root, candidate, false)
}

pub fn confined_path_or_root(
    root: &Path,
    candidate: &Path,
) -> Result<PathBuf, PathConfinementError> {
    confined_path(root, candidate, true)
}

pub fn normalized_absolute(path: &Path, base: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    normalize_path(&absolute)
}

pub fn safe_filename_segment(value: &str) -> bool {
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(segment)) if segment == value)
        && components.next().is_none()
}

fn confined_path(
    root: &Path,
    candidate: &Path,
    allow_root: bool,
) -> Result<PathBuf, PathConfinementError> {
    let resolved = resolved_confined_paths(root, candidate)?;
    validate_path_confinement(&resolved, allow_root)?;
    Ok(resolved.candidate)
}

struct ResolvedConfinedPaths {
    root: PathBuf,
    candidate: PathBuf,
}

fn resolved_confined_paths(
    root: &Path,
    candidate: &Path,
) -> Result<ResolvedConfinedPaths, PathConfinementError> {
    let root = canonical_path(root).map_err(PathConfinementError::RootUnavailable)?;
    let absolute_candidate = normalized_absolute(candidate, &root);
    let candidate =
        canonical_path(&absolute_candidate).map_err(PathConfinementError::TargetUnavailable)?;
    Ok(ResolvedConfinedPaths { root, candidate })
}

fn validate_path_confinement(
    resolved: &ResolvedConfinedPaths,
    allow_root: bool,
) -> Result<(), PathConfinementError> {
    if is_confined(&resolved.root, &resolved.candidate, allow_root) {
        Ok(())
    } else {
        Err(PathConfinementError::OutsideRoot {
            root: resolved.root.clone(),
            target: resolved.candidate.clone(),
        })
    }
}

fn canonical_path(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => canonical_missing_path(path),
        Err(error) => Err(error),
    }
}

fn canonical_missing_path(path: &Path) -> io::Result<PathBuf> {
    let normalized = normalize_path(path);
    let (ancestor, suffix) = existing_ancestor(&normalized)?;
    let mut resolved = fs::canonicalize(ancestor)?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn existing_ancestor(path: &Path) -> io::Result<(PathBuf, Vec<OsString>)> {
    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let Some(component) = ancestor.file_name() else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no existing ancestor for {}", path.display()),
            ));
        };
        suffix.push(component.to_os_string());
        ancestor.pop();
    }
    Ok((ancestor, suffix))
}

fn is_confined(root: &Path, candidate: &Path, allow_root: bool) -> bool {
    candidate.starts_with(root) && (allow_root || candidate != root)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        append_normalized_component(&mut normalized, component);
    }
    normalized
}

fn append_normalized_component(path: &mut PathBuf, component: Component<'_>) {
    match component {
        Component::Prefix(prefix) => path.push(prefix.as_os_str()),
        Component::RootDir => path.push(std::path::MAIN_SEPARATOR.to_string()),
        Component::CurDir => {}
        Component::ParentDir => {
            path.pop();
        }
        Component::Normal(value) => path.push(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expand_home_uses_home_when_available() {
        assert_eq!(
            expand_home("~/project", Some("/home/tester")),
            PathBuf::from("/home/tester").join("project")
        );
        assert_eq!(
            expand_home("~", Some("/home/tester")),
            PathBuf::from("/home/tester")
        );
    }

    #[test]
    fn expand_home_preserves_tilde_without_home() {
        assert_eq!(
            expand_home("~/project", None),
            PathBuf::from("~").join("project")
        );
        assert_eq!(
            expand_home("/tmp/project", None),
            PathBuf::from("/tmp/project")
        );
    }

    #[test]
    fn provider_data_dir_uses_data_root_with_claude_leaf() {
        assert_eq!(
            provider_data_dir(&json!({ "data_root": "/var/lib/provider" })).unwrap(),
            PathBuf::from("/var/lib/provider").join("claude")
        );
    }

    #[test]
    fn provider_data_dir_requires_host_data_root() {
        let error = provider_data_dir(&json!({})).unwrap_err();
        assert_eq!(error.code.as_ref(), "missing_host_data_root");
    }
}
