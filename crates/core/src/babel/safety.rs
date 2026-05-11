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
        EvalPolicy::NoExport => EffectivePolicy::Allow, // allowed interactively
        EvalPolicy::Yes => {
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

    #[test]
    fn no_export_allows_interactive() {
        let policy = effective_eval_policy(&EvalPolicy::NoExport, None, &[], true);
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
