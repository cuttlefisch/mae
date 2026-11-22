//! Path confinement: where a write to `path` really lands, and whether that
//! is inside `root`.
//!
//! @ai-caution: [security] The one implementation for every "may this AI
//! effect write here?" check that confines to a directory (the self-test
//! sandbox, the guidance export). Judge containment on the RESOLVED path:
//! the deepest ancestor of the target that exists, with every symlink
//! resolved. The rest of the path is created below that ancestor, and with no
//! `..` it cannot climb out of it. Comparing the unresolved path instead lets
//! `<root>/../x/file` through whenever `<root>/../x` does not exist, because
//! `Path::starts_with` compares components lexically, `..` included (#967).

use std::path::{Component, Path, PathBuf};

/// Why a target was refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Escape {
    /// The path has a `..` component (or is empty), so where it lands depends
    /// on directories that may not exist yet.
    ParentDir,
    /// The deepest existing ancestor resolves outside the root (shown).
    Outside(PathBuf),
    /// The root or that ancestor could not be resolved (shown: the reason).
    Unresolvable(String),
}

/// Where a write to `path` lands, if that is inside `root`: `root.join(path)`
/// for a relative path, `path` itself for an absolute one. The root itself is
/// not "inside" it.
pub(crate) fn contained(root: &Path, path: &Path) -> Result<PathBuf, Escape> {
    let plain = !path.as_os_str().is_empty()
        && path.components().all(|c| {
            matches!(
                c,
                Component::Normal(_)
                    | Component::CurDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        });
    if !plain {
        return Err(Escape::ParentDir);
    }
    let out = root.join(path);
    let root_real = root
        .canonicalize()
        .map_err(|e| Escape::Unresolvable(format!("{}: {e}", root.display())))?;
    // `symlink_metadata`, not `exists`: a dangling link is an existing entry,
    // and canonicalizing it fails, so it is refused rather than written through.
    let existing = out
        .ancestors()
        .find(|a| a.symlink_metadata().is_ok())
        .unwrap_or(root);
    let existing_real = existing
        .canonicalize()
        .map_err(|e| Escape::Unresolvable(format!("{}: {e}", existing.display())))?;
    if !existing_real.starts_with(&root_real) || out_is_root(&out, &existing_real, &root_real) {
        return Err(Escape::Outside(existing_real));
    }
    Ok(out)
}

/// `out` names the root itself (only possible when it exists, so it is its
/// own deepest existing ancestor).
fn out_is_root(out: &Path, existing_real: &Path, root_real: &Path) -> bool {
    existing_real == root_real && out.symlink_metadata().is_ok()
}

/// Whether the entry at `path` is itself a symbolic link. A confined write
/// refuses one: writing through it would land wherever it points, and even a
/// link to somewhere inside the root turns a deliberate link into a copy.
pub(crate) fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
}

#[cfg(test)]
#[path = "path_confine_tests.rs"]
mod tests;
