//! Validation for the identifiers MAE interpolates into Datalog.
//!
//! @ai-caution: [datalog-injection] CozoScript has no prepared-statement form
//! for a *relation* or a bare identifier, so several query sites build their
//! script with `format!`. Every such site must run its caller-supplied
//! identifier through [`valid_node_id`] first. The alternative that grew up
//! here instead — a `replace('\'', "")` at each site — is worse than useless:
//! it strips single quotes only, and the one site that used double quotes
//! (`kb_view_query`) stripped nothing at all, so a caller-supplied `view_id`
//! escaped its literal and reached `raw_query` with arbitrary Datalog at
//! `ReadOnly` tier. One mechanism, not six (principle #7/#8).
//!
//! Values that are genuinely *data* rather than identifiers should use
//! `run_immut_params` binding instead of this; this exists for the positions
//! binding cannot reach.

/// The characters a KB node id may contain.
///
/// Deliberately an allow-list, not a deny-list: MAE's id namespaces are
/// `cmd:`, `concept:`, `lesson:`, `scheme:`, `option:`, `category:`, `task:`,
/// `view:` and `meta:` followed by a slug, plus block suffixes like
/// `concept:buffer#3` and ADR-105 addresses. None of them needs a quote, a
/// backslash, whitespace or a control character — and those are precisely what
/// an injection needs.
fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.' | '#' | '/' | '+')
}

/// True if `id` is safe to interpolate into a CozoScript string literal.
///
/// Rejects the empty string, anything over [`MAX_ID_LEN`], and any character
/// outside [`is_id_char`] — notably `"`, `'`, `\`, newlines and NUL.
pub fn valid_node_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.chars().all(is_id_char)
}

/// Upper bound on an interpolated identifier.
///
/// Not a security property on its own — [`valid_node_id`]'s character set is
/// what prevents injection — but it keeps a hostile caller from turning an id
/// into a multi-megabyte script fragment that costs parse time before it fails.
pub const MAX_ID_LEN: usize = 512;

/// Validate an identifier bound for a Datalog literal, or explain why not.
///
/// The error is deliberately generic about the offending character: naming it
/// would tell a prober exactly which byte to try next.
pub fn check_node_id(id: &str) -> Result<(), String> {
    if valid_node_id(id) {
        Ok(())
    } else {
        Err(format!(
            "invalid id: must be 1-{MAX_ID_LEN} characters of [A-Za-z0-9:._#/+-]"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_node_ids_are_accepted() {
        for id in [
            "view:kanban",
            "concept:buffer",
            "concept:buffer#3",
            "cmd:save-buffer",
            "task:2026-08-24_write-adr",
            "meta:index",
            "kbn:my-kb:concept:a",
            "a",
            &"x".repeat(MAX_ID_LEN),
        ] {
            assert!(
                valid_node_id(id),
                "{id:?} is a real id and must be accepted"
            );
        }
    }

    /// The attacker's test. Each of these escapes a `"…"` literal, a `'…'`
    /// literal, or terminates the statement — which is what turned
    /// `kb_view_query`'s `format!` into arbitrary Datalog execution.
    #[test]
    fn injection_attempts_are_rejected() {
        for id in [
            r#"x" or id != ""#,
            r#"view:kanban" ; :rm nodes {id} ; ?[x] := x = ""#,
            "x' or 'a'='a",
            r#"x\" "#,
            "x\nid = \"y",
            "x\u{0}y",
            "x y",
            "",
            &"x".repeat(MAX_ID_LEN + 1),
            "?[id] := *nodes{id}",
            "*views{id}",
        ] {
            assert!(
                !valid_node_id(id),
                "{id:?} must be rejected — it can escape a Datalog literal"
            );
        }
    }

    /// Round-trip property: anything accepted survives interpolation into a
    /// double-quoted literal without changing the literal's structure, i.e. the
    /// interpolated script has exactly the two quotes we wrote.
    #[test]
    fn accepted_ids_cannot_change_the_literal_structure() {
        for id in ["view:kanban", "concept:buffer#3", "kbn:kb-a:concept:x"] {
            let script = format!(r#"?[t] := *views{{id, t}}, id = "{id}""#);
            assert_eq!(
                script.matches('"').count(),
                2,
                "interpolating {id:?} changed the quote structure: {script}"
            );
            assert!(!script.contains('\\'), "unexpected escape in {script}");
        }
    }
}
