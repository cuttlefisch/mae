//! One canonical spelling for a KB directory (#832).
//!
//! A directory can be named many ways — a symlink, a relative path, `..`, the
//! macOS `/var` → `/private/var` link, Windows' `\\?\` from `canonicalize()`.
//! KB code that compares or keys on the SPELLING instead of the directory
//! breaks on every platform but the one it was written on (#828's two data-loss
//! bugs were visible only on macOS and Windows CI). Store and compare paths
//! through [`canonical_lenient`].

use std::path::{Path, PathBuf};

/// `path` with every existing prefix canonicalised, so a symlinked or `..`
/// spelling of a directory matches its real location even when the tail does
/// not exist yet (a dailies dir is created on first use; a moved project's old
/// root no longer exists). Falls back to `path` unchanged when no prefix exists.
pub fn canonical_lenient(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(c) = cur.canonicalize() {
            return tail
                .iter()
                .rev()
                .fold(c, |acc: PathBuf, part| acc.join(part));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_directory_resolves_to_its_real_path() {
        let d = tempfile::TempDir::new().unwrap();
        assert_eq!(
            canonical_lenient(d.path()),
            d.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn a_missing_tail_is_kept_under_the_canonical_existing_prefix() {
        let d = tempfile::TempDir::new().unwrap();
        let wanted = d.path().canonicalize().unwrap().join("not").join("yet");
        assert_eq!(canonical_lenient(&d.path().join("not").join("yet")), wanted);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_spelling_resolves_to_the_same_directory() {
        let real = tempfile::TempDir::new().unwrap();
        let links = tempfile::TempDir::new().unwrap();
        let alias = links.path().join("alias");
        std::os::unix::fs::symlink(real.path(), &alias).unwrap();
        assert_eq!(canonical_lenient(&alias), canonical_lenient(real.path()));
        assert_eq!(
            canonical_lenient(&alias.join("daily")),
            canonical_lenient(&real.path().join("daily")),
            "…including a tail that does not exist yet"
        );
    }
}
