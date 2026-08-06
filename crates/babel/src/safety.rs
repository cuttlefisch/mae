//! Babel execution safety — eval policies and trust management.

use std::path::Path;

use super::EvalPolicy;

/// Check if a file path is trusted for babel execution.
/// Uses simple prefix/suffix matching (not full glob).
pub fn is_trusted_path(file_path: &Path, trust_patterns: &[String]) -> bool {
    let path_str = file_path.to_string_lossy();
    for pattern in trust_patterns {
        if matches_trust_pattern(&path_str, pattern) {
            return true;
        }
    }
    false
}

/// Simple pattern matching for trust paths.
/// Supports `*` as wildcard at start/end, and exact prefix matching.
fn matches_trust_pattern(path: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        return path.starts_with(prefix) || path.starts_with(&format!("{}/", prefix));
    }
    if let Some(suffix) = pattern.strip_prefix("*/") {
        return path.ends_with(suffix);
    }
    if let Some(ext) = pattern.strip_prefix("*.") {
        return path.ends_with(&format!(".{}", ext));
    }
    // Exact directory prefix
    if pattern.ends_with('/') {
        return path.starts_with(pattern);
    }
    path == pattern
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
        assert!(matches_trust_pattern("/any/path", "*"));
    }

    #[test]
    fn trust_pattern_dir_prefix() {
        assert!(matches_trust_pattern("/tmp/test.org", "/tmp/*"));
        assert!(!matches_trust_pattern("/home/test.org", "/tmp/*"));
    }

    #[test]
    fn trust_pattern_extension() {
        assert!(matches_trust_pattern("/any/file.org", "*.org"));
        assert!(!matches_trust_pattern("/any/file.txt", "*.org"));
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
