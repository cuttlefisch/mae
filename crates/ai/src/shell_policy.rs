//! The `shell_exec` argument + blocklist policy, shared by both callers.
//!
//! `shell_exec` has two implementations for real reasons — the embedded
//! `AgentSession` runs it on tokio (`session::run_loop`), MCP/non-session
//! callers run it synchronously (`executor::shell_exec`). What must NOT differ
//! between them is the *policy*: which commands are refused and how long a
//! command is allowed to run. Both had their own hand-copied blocklist array
//! and their own argument parsing, and the parsing had already drifted from the
//! advertised tool schema (audit #590.3).
//!
//! @ai-caution: [security] Any new refusal rule or argument goes HERE, not in
//! one of the two call sites. A rule added to only one of them is a rule the
//! model can route around by picking the other surface.
//!
//! This is deliberately "defense in depth, not a sandbox" — the blocklist is
//! substring-based and bypassable by design; see SECURITY.md. Sandbox
//! confinement is a separate, stronger mechanism applied by the dispatcher.

/// Command substrings refused outright.
///
/// Not a security boundary — a trivially-obfuscated equivalent gets through.
/// It exists to stop an *accident*: a model that has talked itself into
/// `rm -rf /` should not get one shot at it.
const BLOCKED_PATTERNS: &[&str] = &[
    "rm -rf /", "rm -fr /", "mkfs.", "dd if=", ":(){", // fork bomb
    ">(){ :",
];

/// Default command timeout when the caller specifies none.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Hard ceiling on any caller-supplied timeout.
pub const MAX_TIMEOUT_MS: u64 = 120_000;

/// The blocked pattern `command` contains, if any.
pub fn blocked_pattern(command: &str) -> Option<&'static str> {
    BLOCKED_PATTERNS
        .iter()
        .find(|p| command.contains(**p))
        .copied()
}

/// Resolve the timeout for a `shell_exec` call from its arguments.
///
/// The advertised schema names `timeout_ms` (`tools/shell_tools.rs`), but both
/// implementations only ever read `timeout_secs` (audit #590.3) — so a model
/// following the published schema and passing `timeout_ms: 5000` silently got
/// the 30s default, and one passing `timeout_ms: 120000` got 30s rather than
/// the two minutes it asked for. The schema is the contract with the model, so
/// `timeout_ms` wins; `timeout_secs` stays accepted as an undocumented alias so
/// any caller already relying on the implemented-but-unadvertised name keeps
/// working. Always clamped to [`MAX_TIMEOUT_MS`].
pub fn timeout_from_args(args: &serde_json::Value) -> std::time::Duration {
    let ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            args.get("timeout_secs")
                .and_then(|v| v.as_u64())
                // Saturating: a nonsense `timeout_secs: u64::MAX` must clamp,
                // not wrap to a tiny millisecond value.
                .map(|s| s.saturating_mul(1000))
        })
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(MAX_TIMEOUT_MS);
    std::time::Duration::from_millis(ms)
}

/// Human-readable rendering of a resolved timeout, for a timeout message.
pub fn describe_timeout(d: std::time::Duration) -> String {
    let ms = d.as_millis();
    if ms.is_multiple_of(1000) {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Audit #590.3 — the advertised argument was never read. A model that
    /// follows the published schema must actually get the timeout it asked
    /// for, in both directions (shorter AND longer than the default), or the
    /// argument is decoration.
    #[test]
    fn the_advertised_timeout_ms_argument_is_honored() {
        assert_eq!(
            timeout_from_args(&json!({"timeout_ms": 5000})),
            std::time::Duration::from_secs(5),
            "a model asking for LESS than the default must get less"
        );
        assert_eq!(
            timeout_from_args(&json!({"timeout_ms": 90_000})),
            std::time::Duration::from_secs(90),
            "a model asking for MORE than the default must get more"
        );
        assert_eq!(
            timeout_from_args(&json!({"timeout_ms": 250})),
            std::time::Duration::from_millis(250),
            "sub-second precision is the whole point of a _ms argument"
        );
    }

    #[test]
    fn the_undocumented_timeout_secs_alias_still_works() {
        assert_eq!(
            timeout_from_args(&json!({"timeout_secs": 7})),
            std::time::Duration::from_secs(7)
        );
        // `timeout_ms` wins when both are present — it is the advertised name.
        assert_eq!(
            timeout_from_args(&json!({"timeout_ms": 1000, "timeout_secs": 90})),
            std::time::Duration::from_secs(1)
        );
    }

    /// The ceiling must hold against values chosen to break the arithmetic,
    /// not just against a plausible-but-large number.
    #[test]
    fn a_hostile_timeout_cannot_exceed_or_wrap_past_the_ceiling() {
        for args in [
            json!({"timeout_ms": u64::MAX}),
            json!({"timeout_secs": u64::MAX}),
            json!({"timeout_ms": MAX_TIMEOUT_MS + 1}),
            json!({"timeout_secs": 999_999_999}),
            // Non-integer / wrong-typed values fall back to the default rather
            // than being coerced into something surprising.
            json!({"timeout_ms": -1}),
            json!({"timeout_ms": "forever"}),
            json!({"timeout_ms": 1.5}),
        ] {
            let d = timeout_from_args(&args);
            assert!(
                d <= std::time::Duration::from_millis(MAX_TIMEOUT_MS),
                "{args} produced {d:?}, past the ceiling"
            );
            assert!(d > std::time::Duration::ZERO, "{args} produced no timeout");
        }
        assert_eq!(
            timeout_from_args(&json!({})),
            std::time::Duration::from_millis(DEFAULT_TIMEOUT_MS)
        );
    }

    #[test]
    fn the_blocklist_matches_and_is_shared() {
        assert_eq!(
            blocked_pattern("sudo rm -rf / --no-preserve-root"),
            Some("rm -rf /")
        );
        assert_eq!(
            blocked_pattern("dd if=/dev/zero of=/dev/sda"),
            Some("dd if=")
        );
        assert_eq!(blocked_pattern(":(){ :|:& };:"), Some(":(){"));
        assert_eq!(blocked_pattern("ls -la"), None);
        // Documented as bypassable — pinned so nobody mistakes it for a
        // sandbox and removes the real confinement in favour of it.
        assert_eq!(
            blocked_pattern("rm  -rf  /"),
            None,
            "substring matching is defense in depth, not a boundary (SECURITY.md)"
        );
    }

    #[test]
    fn describe_timeout_reads_naturally_in_both_units() {
        assert_eq!(describe_timeout(std::time::Duration::from_secs(30)), "30s");
        assert_eq!(
            describe_timeout(std::time::Duration::from_millis(250)),
            "250ms"
        );
    }
}
