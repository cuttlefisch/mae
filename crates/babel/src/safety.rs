//! Babel execution safety — eval policies and trust management.

use std::path::{Component, Path, PathBuf};

use super::EvalPolicy;

/// Check if a file path is trusted for babel execution.
/// Uses simple prefix/suffix matching (not full glob).
///
/// @ai-caution: [security] Normalization happens HERE, before any pattern is
/// consulted, and the patterns are normalized the same way. `babel_trust_paths`
/// is a trust boundary: matching the caller's raw string would let
/// `/tmp/../etc/x.org` satisfy `/tmp/*` while naming a file outside `/tmp`.
pub fn is_trusted_path(file_path: &Path, trust_patterns: &[String]) -> bool {
    let normalized = normalize_lexically(file_path);
    for pattern in trust_patterns {
        if matches_trust_pattern(&normalized, pattern) {
            return true;
        }
    }
    false
}

/// Resolve `.` and `..` components lexically, without touching the filesystem.
///
/// Lexical rather than [`std::fs::canonicalize`] on purpose: the path may name
/// a file that does not exist yet (a tangle target, an unsaved buffer), and a
/// canonicalize failure must not silently degrade to "compare the raw string",
/// which is the bug this exists to prevent.
///
/// This does not resolve symlinks, so a symlink inside a trusted directory
/// pointing outside it is still trusted — trust is granted per directory by the
/// user, and the component boundary in [`is_within_dir`] is what stops the
/// name-prefix escape.
///
/// @ai-caution: [cross-platform] Goes through [`Path::components`], NOT string
/// splitting. The first version of this split on `'/'` and decided absoluteness
/// with `starts_with('/')`, which is POSIX-only: on Windows a path is
/// `C:\Users\…`, so it produced ONE component and `is_absolute == false`, and
/// babel trust matching silently stopped working there. Caught by CI's
/// `stable / test (windows)` leg, never by the Linux suite — the exact failure
/// CLAUDE.md principle #13 exists for. `Components` handles both separators and
/// the drive prefix for free; do not reintroduce a separator literal here.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // `pop()` fails at a root or on an empty relative path. At a
                // root, dropping the `..` mirrors the kernel's `/..` == `/`;
                // on a relative path there is nothing above, so keep the `..`
                // so it cannot be matched away.
                if !out.pop() && out.as_os_str().is_empty() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `/tmp`, because prefix-of-string is not prefix-of-path.
fn is_within_dir(path: &Path, dir: &Path) -> bool {
    // Component-wise by construction: `Path::starts_with` compares whole
    // components, so `/tmp` does not match `/tmpevil` and no trailing-separator
    // bookkeeping is needed. The previous string `strip_prefix` needed that
    // bookkeeping, got it wrong in one of two disjuncts, and was POSIX-only.
    // An empty `dir` (pattern `/`) is every path's prefix, which is what the
    // whole-filesystem pattern should mean.
    // STRICT descendant: the trusted directory is not itself a file "inside"
    // it. `Path::starts_with` counts a path as its own prefix, so `/tmp` would
    // otherwise satisfy `/tmp/*` — the string version required a trailing
    // separator in the remainder and did not. Preserved deliberately; the
    // existing boundary test asserts it.
    path != dir && path.starts_with(dir)
}

/// Simple pattern matching for trust paths.
/// Supports `*` as wildcard at start/end, and exact prefix matching.
///
/// `path` is expected to be normalized already (see [`is_trusted_path`]); the
/// pattern is normalized here so a user-written `~/notes/../notes/*` behaves
/// the same as the path side.
fn matches_trust_pattern(path: &Path, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Directory containment — the security-relevant cases. Both sides go
    // through the same lexical normalization and the same component-wise
    // boundary check.
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return is_within_dir(path, &normalize_lexically(Path::new(prefix)));
    }
    if pattern.ends_with('/') {
        return is_within_dir(path, &normalize_lexically(Path::new(pattern)));
    }
    // Suffix forms stay textual: they are not boundary-sensitive in the way
    // containment is, and `*.org` means "this extension" regardless of where
    // the file lives.
    let as_text = path.to_string_lossy();
    if let Some(suffix) = pattern.strip_prefix("*/") {
        return as_text.ends_with(suffix);
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        return as_text.ends_with(&format!(".{ext}"));
    }
    path == normalize_lexically(Path::new(pattern))
}

/// Determine the effective eval policy for a block.
pub fn effective_eval_policy(
    block_policy: &EvalPolicy,
    file_path: Option<&Path>,
    trust_patterns: &[String],
    global_confirm: bool,
) -> EffectivePolicy {
    match block_policy {
        EvalPolicy::Never => EffectivePolicy::Blocked,
        // @ai-caution: [security] `no-export` means "evaluate normally, just
        // not during export" — it is NOT an exemption from confirmation. This
        // arm used to return `Allow` directly, short-circuiting BOTH
        // `global_confirm` and `trust_patterns` below, so a block marked
        // `:eval no-export` in a peer's shared-KB node or a cloned repo's .org
        // ran with no prompt at stock settings (`babel_confirm = true`, no
        // trust paths). Falling through to the `Yes` arm restores the real org
        // semantics and puts it back behind the same two gates as every other
        // executable block.
        EvalPolicy::NoExport | EvalPolicy::Yes => {
            if !global_confirm {
                return EffectivePolicy::Allow;
            }
            if let Some(path) = file_path {
                if is_trusted_path(path, trust_patterns) {
                    return EffectivePolicy::Allow;
                }
            }
            EffectivePolicy::NeedsConfirmation
        }
        EvalPolicy::Query => EffectivePolicy::NeedsConfirmation,
    }
}

/// The effective policy after considering trust and global settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectivePolicy {
    Allow,
    NeedsConfirmation,
    Blocked,
}

#[cfg(test)]
mod tests {
    /// Mirrors production: `is_trusted_path` normalizes the PATH before any
    /// pattern is consulted, so tests must too — otherwise they would exercise
    /// a code path no caller can reach.
    fn matches(path: &str, pattern: &str) -> bool {
        super::matches_trust_pattern(
            &super::normalize_lexically(std::path::Path::new(path)),
            pattern,
        )
    }

    use super::*;

    #[test]
    fn never_always_blocked() {
        let policy = effective_eval_policy(
            &EvalPolicy::Never,
            Some(Path::new("/tmp/test.org")),
            &[],
            true,
        );
        assert_eq!(policy, EffectivePolicy::Blocked);
    }

    #[test]
    fn yes_with_no_confirm() {
        let policy = effective_eval_policy(
            &EvalPolicy::Yes,
            Some(Path::new("/tmp/test.org")),
            &[],
            false,
        );
        assert_eq!(policy, EffectivePolicy::Allow);
    }

    #[test]
    fn yes_with_confirm_untrusted() {
        let policy = effective_eval_policy(
            &EvalPolicy::Yes,
            Some(Path::new("/tmp/test.org")),
            &[],
            true,
        );
        assert_eq!(policy, EffectivePolicy::NeedsConfirmation);
    }

    #[test]
    fn yes_with_confirm_trusted() {
        let policy = effective_eval_policy(
            &EvalPolicy::Yes,
            Some(Path::new("/tmp/test.org")),
            &["/tmp/*".to_string()],
            true,
        );
        assert_eq!(policy, EffectivePolicy::Allow);
    }

    #[test]
    fn query_always_needs_confirmation() {
        let policy = effective_eval_policy(
            &EvalPolicy::Query,
            Some(Path::new("/trusted/test.org")),
            &["/trusted/*".to_string()],
            false,
        );
        assert_eq!(policy, EffectivePolicy::NeedsConfirmation);
    }

    /// Renamed and corrected from `no_export_allows_interactive`, which asserted
    /// `Allow` here and so encoded the bypass as a requirement: it locked in the
    /// behaviour that let a peer-authored `:eval no-export` block run with no
    /// prompt at stock settings. `no-export` is about EXPORT, not about
    /// confirmation, so with `babel_confirm = true` it must ask like any other
    /// executable block.
    #[test]
    fn no_export_still_asks_when_confirmation_is_on() {
        let policy = effective_eval_policy(&EvalPolicy::NoExport, None, &[], true);
        assert_eq!(policy, EffectivePolicy::NeedsConfirmation);
    }

    /// The other half: with confirmation turned off, `no-export` runs — same as
    /// `:eval yes`. Without this, the change above could be "satisfied" by
    /// blocking `no-export` outright, which would break every legitimate use.
    #[test]
    fn no_export_runs_when_confirmation_is_off() {
        let policy = effective_eval_policy(&EvalPolicy::NoExport, None, &[], false);
        assert_eq!(policy, EffectivePolicy::Allow);
    }

    #[test]
    fn trust_pattern_wildcard() {
        assert!(matches("/any/path", "*"));
    }

    /// Replaces `trust_pattern_dir_prefix`, which picked the only two inputs
    /// that pass whether or not the boundary check exists (`/tmp/test.org`
    /// under `/tmp/*`, and `/home/test.org`, which shares no prefix at all) —
    /// evidence of nothing. The `dir/*` form must mean "inside `dir`", so the
    /// oracle is the sibling whose name merely *starts with* the trusted
    /// directory's name.
    #[test]
    fn dir_prefix_trusts_only_paths_inside_the_directory() {
        // Still trusted — the whole point of the pattern.
        assert!(matches("/tmp/test.org", "/tmp/*"));
        assert!(matches("/tmp/nested/deep/test.org", "/tmp/*"));

        // Sibling-prefix escape: `/tmpevil` is NOT inside `/tmp`.
        assert!(!matches("/tmpevil/x.org", "/tmp/*"));
        assert!(!matches("/tmp-other/x.org", "/tmp/*"));
        assert!(!matches("/tmpevil", "/tmp/*"));

        // Unrelated path.
        assert!(!matches("/home/test.org", "/tmp/*"));

        // The directory itself is not a file inside it.
        assert!(!matches("/tmp", "/tmp/*"));
    }

    /// Same boundary rule for the bare-directory form (`pattern` ending in
    /// `/`), which shares the prefix-matching code path.
    #[test]
    fn bare_dir_prefix_trusts_only_paths_inside_the_directory() {
        assert!(matches("/tmp/test.org", "/tmp/"));
        assert!(!matches("/tmpevil/x.org", "/tmp/"));
    }

    /// `..` must not walk out of a trusted directory. Without normalization,
    /// `/tmp/../etc/x.org` matches `/tmp/*` by string prefix while naming a
    /// file that is not in `/tmp` at all — and it is `is_trusted_path`, the
    /// public entry point, that must hold this, since that is where a real
    /// `&Path` from an opened buffer arrives.
    #[test]
    fn traversal_out_of_a_trusted_directory_is_not_trusted() {
        let trusted = ["/tmp/*".to_string()];
        assert!(!is_trusted_path(Path::new("/tmp/../etc/x.org"), &trusted));
        assert!(!is_trusted_path(
            Path::new("/tmp/sub/../../etc/x.org"),
            &trusted
        ));
        // Traversal that stays inside is still trusted.
        assert!(is_trusted_path(Path::new("/tmp/sub/../a.org"), &trusted));
        assert!(is_trusted_path(Path::new("/tmp/./a.org"), &trusted));
    }

    /// The escape reaches the real decision, not just the matcher: an
    /// `:eval yes` block in `/tmpevil/x.org` must still prompt when the user
    /// trusted `/tmp/*`.
    #[test]
    fn sibling_prefix_file_still_needs_confirmation() {
        assert_eq!(
            effective_eval_policy(
                &EvalPolicy::Yes,
                Some(Path::new("/tmpevil/x.org")),
                &["/tmp/*".to_string()],
                true,
            ),
            EffectivePolicy::NeedsConfirmation,
            "a file merely sharing a name prefix with a trusted directory must not be trusted"
        );
    }

    /// Property: normalizing twice changes nothing, and whatever
    /// `is_within_dir` accepts really does live under the directory. If either
    /// held only for hand-picked inputs the boundary check would be decorative.
    #[test]
    fn normalization_and_containment_hold_as_properties() {
        let cases = [
            "/tmp/a.org",
            "/tmp//a.org",
            "/tmp/./a.org",
            "/tmp/sub/../a.org",
            "/tmp/../etc/a.org",
            "/../etc/a.org",
            "/tmpevil/a.org",
            "relative/a.org",
            "../escape/a.org",
            "/",
            "",
        ];
        let tmp = std::path::Path::new("/tmp");
        for raw in cases {
            let once = normalize_lexically(std::path::Path::new(raw));
            assert_eq!(
                normalize_lexically(&once),
                once,
                "normalization must be idempotent for {raw:?}"
            );
            let text = once.to_string_lossy();
            assert!(
                !text.contains("/./") && !text.contains("//"),
                "{raw:?} normalized to {once:?}, which still carries redundant components"
            );
            if is_within_dir(&once, tmp) {
                assert!(
                    once.strip_prefix(tmp).is_ok(),
                    "{once:?} was accepted as inside /tmp but does not live there"
                );
            }
        }
    }

    /// Native-path coverage, and the gap that let the POSIX-only version ship.
    ///
    /// Every other test here hardcodes `/tmp/...`, which on Windows is not an
    /// absolute path at all — so the whole boundary suite passed there while
    /// trust matching was completely broken. The defect was caught only by a
    /// `mae-core` editor test, and only on CI's `stable / test (windows)` leg,
    /// two crates away from the code at fault.
    ///
    /// This builds the directory and the pattern the way a real caller does —
    /// from `std::env::temp_dir()`, whatever shape that has on this platform —
    /// so the same assertion is meaningful on Linux, macOS and Windows.
    #[test]
    fn trust_matching_works_with_native_platform_paths() {
        let dir = std::env::temp_dir().join("mae-babel-native-path-check");
        let file = dir.join("notes.org");
        let sibling = dir
            .with_file_name("mae-babel-native-path-checkEVIL")
            .join("x.org");

        // A user writes the pattern from the directory they see.
        let pattern = format!("{}/*", dir.display());

        assert!(
            is_trusted_path(&file, std::slice::from_ref(&pattern)),
            "a file inside the trusted dir must be trusted on this platform; \
             dir={dir:?} pattern={pattern:?}"
        );
        assert!(
            !is_trusted_path(&sibling, std::slice::from_ref(&pattern)),
            "the sibling-prefix escape must still be refused with native paths; \
             sibling={sibling:?} pattern={pattern:?}"
        );
        assert!(
            !is_trusted_path(&dir, &[pattern]),
            "the trusted directory is not itself a file inside it"
        );
    }

    /// The Windows separator explicitly, so the regression cannot come back
    /// through a hand-rolled `'/'` literal. `Path` accepts both separators on
    /// Windows; a string split on `'/'` sees one component and fails.
    #[cfg(windows)]
    #[test]
    fn backslash_paths_normalize_and_bound_correctly() {
        use std::path::Path;
        let normalized = normalize_lexically(Path::new(r"C:\work\sub\..\a.org"));
        assert_eq!(normalized, Path::new(r"C:\work\a.org"));
        assert!(is_within_dir(
            Path::new(r"C:\work\a.org"),
            Path::new(r"C:\work")
        ));
        assert!(!is_within_dir(
            Path::new(r"C:\workEVIL\a.org"),
            Path::new(r"C:\work")
        ));
    }

    #[test]
    fn trust_pattern_extension() {
        assert!(matches("/any/file.org", "*.org"));
        assert!(!matches("/any/file.txt", "*.org"));
    }
}

#[cfg(test)]
mod no_export_gate_tests {
    use super::*;
    use std::path::Path;

    /// `:eval no-export` used to return `Allow` before `global_confirm` or
    /// `trust_patterns` were consulted — the only `EvalPolicy` variant that
    /// skipped both. A `#+begin_src bash :eval no-export` block in a peer's
    /// shared-KB node, or a cloned repo's `.org`, therefore ran with no prompt
    /// at stock settings.
    ///
    /// Real org semantics are "evaluate normally, but not during export", so it
    /// must behave exactly like `Yes`. The oracle is that the two variants agree
    /// across the whole input space, not that `no-export` returns one specific
    /// value — that way a future change to `Yes`'s gating cannot silently
    /// re-open the gap.
    #[test]
    fn no_export_is_gated_exactly_like_yes() {
        let trusted = ["/trusted/*".to_string()];
        let cases = [
            (None, &[][..], true),
            (None, &[][..], false),
            (Some(Path::new("/untrusted/a.org")), &trusted[..], true),
            (Some(Path::new("/trusted/a.org")), &trusted[..], false),
            (Some(Path::new("/trusted/a.org")), &trusted[..], true),
            (Some(Path::new("/untrusted/a.org")), &[][..], true),
        ];
        for (path, patterns, confirm) in cases {
            assert_eq!(
                effective_eval_policy(&EvalPolicy::NoExport, path, patterns, confirm),
                effective_eval_policy(&EvalPolicy::Yes, path, patterns, confirm),
                "no-export must be gated identically to :eval yes \
                 (path={path:?}, confirm={confirm})"
            );
        }
    }

    /// The specific case that was exploitable: stock settings, untrusted file.
    #[test]
    fn no_export_needs_confirmation_at_stock_settings() {
        assert_eq!(
            effective_eval_policy(
                &EvalPolicy::NoExport,
                Some(Path::new("/somebody-elses/notes.org")),
                &[],
                true, // babel_confirm defaults to true
            ),
            EffectivePolicy::NeedsConfirmation,
            "a peer-authored no-export block must prompt, not run silently"
        );
    }
}
