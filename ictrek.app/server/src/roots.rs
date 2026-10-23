//! Authorized directories and path resolution.
//!
//! VOS mounts the directories a user may reach beneath `VOS_APP_EXPOSED_PATH`. The shape is
//! `public/<folder>` and `users/<username>/data`; everything else in that tree is scaffolding and
//! must stay unreachable. Every request path is resolved through this module, so a new endpoint
//! cannot accidentally invent its own traversal rules.

use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

use axum::http::StatusCode;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct Root {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

impl Root {
    /// A directory under `users/<username>`, as opposed to a public one. The MCP workspace prefers
    /// it so an agent writes where its own user can find the result.
    ///
    /// Matched by path segment, not by prefix: the exposed tree may be reached through extra
    /// scaffolding (`<something>/users/<name>/data`), and a public folder could legitimately be
    /// named `users`.
    pub fn is_personal_of(&self, username: &str) -> bool {
        let segments: Vec<&str> = self.name.split('/').collect();
        segments.windows(2).any(|pair| pair[0] == "users" && pair[1] == username)
    }
}

fn root_id(path: &Path) -> String {
    format!("{:x}", Sha256::digest(path.to_string_lossy().as_bytes()))
}

/// The mount tree is supplied by VOS. Traverse scaffolding only; never descend into another user's
/// data.
pub fn roots(base: &Path, username: &str) -> Vec<Root> {
    fn visit(base: &Path, dir: &Path, username: &str, depth: usize, seen: &mut HashSet<PathBuf>, out: &mut Vec<Root>) {
        if depth > 6 {
            return;
        }
        let Ok(real) = dir.canonicalize() else { return };
        if !real.starts_with(base) || !seen.insert(real) {
            return;
        }
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "public"
            || (name == "data"
                && dir.parent().and_then(Path::file_name).and_then(|v| v.to_str()) == Some(username)
                && dir.parent().and_then(Path::parent).and_then(Path::file_name).and_then(|v| v.to_str()) == Some("users"))
        {
            // VOS may mount only public/<selected-folder>; its parent is scaffolding.
            let mounts = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
            let mounts: HashSet<PathBuf> = mounts
                .lines()
                .filter_map(|line| line.split_whitespace().nth(4))
                .map(|p| PathBuf::from(p.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\")))
                .collect();
            let children: Vec<PathBuf> =
                std::fs::read_dir(dir).ok().into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir() && mounts.contains(p)).collect();
            let paths = if children.is_empty() { vec![dir.to_path_buf()] } else { children };
            for path in paths {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                out.push(Root { id: root_id(rel), name: rel.to_string_lossy().into_owned(), path });
            }
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if name == "users" && entry.file_name() != username {
                continue;
            }
            visit(base, &path, username, depth + 1, seen, out);
        }
    }
    let mut out = Vec::new();
    if let Ok(base) = base.canonicalize() {
        visit(&base, &base, username, 0, &mut HashSet::new(), &mut out);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The directory an agent workspace should be anchored on: the caller's own `users/<name>/data`
/// when it exists, otherwise the first authorized directory. `None` means the user has no grant.
pub fn workspace_root(base: &Path, username: &str) -> Option<Root> {
    let mut authorized = roots(base, username);
    if let Some(index) = authorized.iter().position(|root| root.is_personal_of(username)) {
        return Some(authorized.swap_remove(index));
    }
    authorized.into_iter().next()
}

/// A request-supplied path, refused unless every component is a plain name.
pub fn relative(path: &str, adapter: &str) -> Result<PathBuf> {
    let raw = path.strip_prefix(&format!("{adapter}://")).unwrap_or(path);
    let rel = Path::new(raw);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) || raw.contains('\\') || raw.chars().any(char::is_control) {
        return Err(Error(StatusCode::FORBIDDEN, "Invalid authorized path"));
    }
    Ok(rel.into())
}

/// Resolve one request path beneath the directory the adapter id names.
pub fn resolve(base: &Path, username: &str, adapter: &str, path: &str, create: bool) -> Result<PathBuf> {
    let id = adapter.strip_prefix("x_").ok_or(Error(StatusCode::FORBIDDEN, "Unknown authorized directory"))?;
    let root = roots(base, username).into_iter().find(|r| r.id == id).ok_or(Error(StatusCode::FORBIDDEN, "Directory is not authorized for this user"))?;
    let base = root.path.canonicalize().map_err(crate::error::io)?;
    let target = base.join(relative(path, adapter)?);
    let check = if create && !target.exists() {
        target.parent().ok_or(Error(StatusCode::FORBIDDEN, "Invalid target"))?.canonicalize().map_err(crate::error::io)?
    } else {
        target.canonicalize().map_err(|_| Error(StatusCode::NOT_FOUND, "Authorized file not found"))?
    };
    if !check.starts_with(&base) {
        return Err(Error(StatusCode::FORBIDDEN, "Path leaves the authorized directory"));
    }
    Ok(if create && !target.exists() { check.join(target.file_name().ok_or(Error(StatusCode::BAD_REQUEST, "Invalid target"))?) } else { check })
}

/// A file or folder name supplied by the client: one plain name, no traversal.
pub fn leaf(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        Err(Error(StatusCode::BAD_REQUEST, "Invalid file name"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_isolate_users_and_stop_at_grants() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["v/public/photos", "v/users/alice/data", "v/users/bob/data"] {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        let found = roots(tmp.path(), "alice");
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|r| !r.name.contains("bob")));
    }

    #[test]
    fn rejects_traversal_and_external_symlinks() {
        assert!(relative("x_root://../outside", "x_root").is_err());
        assert!(relative("/etc/passwd", "x_root").is_err());
        assert!(leaf("../x").is_err());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("v/public");
        std::fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
        let id = &roots(tmp.path(), "alice")[0].id;
        assert!(resolve(tmp.path(), "alice", &format!("x_{id}"), "escape/passwd", false).is_err());
    }

    #[test]
    fn the_workspace_prefers_the_callers_own_directory() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["v/public/photos", "v/users/alice/data"] {
            std::fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        let root = workspace_root(tmp.path(), "alice").unwrap();
        assert!(root.is_personal_of("alice"));
        assert!(root.name.contains("users/alice"));
    }

    #[test]
    fn a_user_without_a_grant_has_no_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("v/users/bob/data")).unwrap();
        assert!(workspace_root(tmp.path(), "alice").is_none());
    }
}
