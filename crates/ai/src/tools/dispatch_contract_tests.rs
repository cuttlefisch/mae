//! Registry-driven guard against "schema and implementation disagree" — the
//! second instance of the #521-era permission-enforcement audit's defect
//! class (~57 findings of "something is registered but cannot actually be
//! reached"): an MCP tool's advertised JSON schema can omit a parameter its
//! implementation unconditionally requires, so the agent has no way to
//! construct a call the impl will accept. Confirmed example: `lsp_workspace_symbol`
//! declared only `query` as a property (and as required), while
//! `execute_lsp_workspace_symbol` (`crates/ai/src/tool_impls/lsp.rs`) also
//! unconditionally requires `language_id` via
//! `.ok_or("Missing 'language_id' argument")?` — a value the schema never
//! told the caller existed.
//!
//! # Approach (static, source-text based — not full parsing)
//!
//! For every tool from `ai_specific_tools`, look for a function named
//! `execute_<tool_name>` in the concatenated source of `tool_impls/*.rs` and
//! `executor/*_exec.rs` + `executor/perf.rs`/`executor/self_test.rs` (the
//! files that hold real tool implementations, as opposed to dispatch
//! scaffolding). If found, extract its body (brace-depth matched) and scan
//! it for the `ok_or("Missing '<name>' ...")` / `ok_or_else(|| "Missing
//! required parameter: <name>")` family of patterns used throughout this
//! codebase to reject a genuinely-required argument. Every name found this
//! way MUST appear in the tool's declared `properties` AND `required` list.
//!
//! # What this does NOT cover (documented, not silently ignored)
//!
//! - Tools with no `execute_<tool_name>` function found by name (dispatched
//!   through some other path — `command_*` registry tools, tools matched
//!   directly by literal name inside `tool_dispatch.rs`'s big match with a
//!   differently-named handler, etc.) are skipped, not silently passed —
//!   `dispatch_contract_test_coverage_is_tracked` pins the current skip list
//!   so a regression (a tool that used to be checkable losing its
//!   `execute_<name>` convention) is caught, and any new tool added to that
//!   skip list must be a deliberate, reviewed choice, not a growing blind
//!   spot.
//! - A "Missing ..." check performed inside a *shared helper* the impl
//!   function calls (e.g. `resolve_buffer_idx`) rather than inline in the
//!   impl's own body is not attributed to that tool — this only sees what's
//!   textually inside the extracted function.
//! - This is a source-text heuristic (brace-depth matching, not a real
//!   parser), so it can misparse a function containing an unbalanced
//!   `{`/`}` inside a string or comment. None of the scanned functions do
//!   today; a false result here is a prompt to look at the offending
//!   function, not to loosen the check.

use std::collections::BTreeSet;

use crate::tools::ai_specific_tools;
use mae_core::OptionRegistry;

/// Source files that hold real tool-implementation bodies (as opposed to
/// dispatch/routing scaffolding like `tool_dispatch.rs`, `permission.rs`,
/// `sandbox.rs`, `grading.rs`, `model_exam.rs`, or the `mod_tests.rs` test
/// module, which this test does not read).
const IMPL_SOURCES: &[(&str, &str)] = &[
    (
        "tool_impls/buffer.rs",
        include_str!("../tool_impls/buffer.rs"),
    ),
    ("tool_impls/dap.rs", include_str!("../tool_impls/dap.rs")),
    (
        "tool_impls/editor_tools.rs",
        include_str!("../tool_impls/editor_tools.rs"),
    ),
    ("tool_impls/file.rs", include_str!("../tool_impls/file.rs")),
    ("tool_impls/git.rs", include_str!("../tool_impls/git.rs")),
    (
        "tool_impls/guidance_export.rs",
        include_str!("../tool_impls/guidance_export.rs"),
    ),
    ("tool_impls/help.rs", include_str!("../tool_impls/help.rs")),
    (
        "tool_impls/image.rs",
        include_str!("../tool_impls/image.rs"),
    ),
    (
        "tool_impls/introspect.rs",
        include_str!("../tool_impls/introspect.rs"),
    ),
    (
        "tool_impls/kb_export_html.rs",
        include_str!("../tool_impls/kb_export_html.rs"),
    ),
    ("tool_impls/kb.rs", include_str!("../tool_impls/kb.rs")),
    ("tool_impls/lsp.rs", include_str!("../tool_impls/lsp.rs")),
    (
        "tool_impls/project.rs",
        include_str!("../tool_impls/project.rs"),
    ),
    (
        "tool_impls/shell.rs",
        include_str!("../tool_impls/shell.rs"),
    ),
    (
        "tool_impls/syntax.rs",
        include_str!("../tool_impls/syntax.rs"),
    ),
    (
        "executor/ai_exec.rs",
        include_str!("../executor/ai_exec.rs"),
    ),
    (
        "executor/collab_exec.rs",
        include_str!("../executor/collab_exec.rs"),
    ),
    (
        "executor/core_exec.rs",
        include_str!("../executor/core_exec.rs"),
    ),
    (
        "executor/dap_exec.rs",
        include_str!("../executor/dap_exec.rs"),
    ),
    (
        "executor/kb_exec.rs",
        include_str!("../executor/kb_exec.rs"),
    ),
    (
        "executor/lsp_exec.rs",
        include_str!("../executor/lsp_exec.rs"),
    ),
    ("executor/perf.rs", include_str!("../executor/perf.rs")),
    (
        "executor/self_test.rs",
        include_str!("../executor/self_test.rs"),
    ),
    (
        "executor/shell_exec.rs",
        include_str!("../executor/shell_exec.rs"),
    ),
    (
        "executor/sync_exec.rs",
        include_str!("../executor/sync_exec.rs"),
    ),
];

/// Find `fn execute_<tool_name>(` (with an optional `pub`/`pub(crate)`
/// prefix, anchored to the start of a line so we don't match a substring
/// inside a longer identifier) across `IMPL_SOURCES` and return its
/// brace-depth-matched body.
fn find_fn_body(fn_name: &str) -> Option<String> {
    let needle_plain = format!("fn {fn_name}(");
    for (_path, src) in IMPL_SOURCES {
        // Anchor: the signature must start a line (after optional
        // indentation + `pub`/`pub(crate)` + whitespace), so we don't match
        // e.g. `fn execute_kb_search_context` when looking for
        // `fn execute_kb_search`.
        let mut search_from = 0usize;
        while let Some(rel) = src[search_from..].find(&needle_plain) {
            let idx = search_from + rel;
            let line_start = src[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0);
            let prefix = src[line_start..idx].trim_start();
            let is_fn_start = prefix.is_empty()
                || prefix.starts_with("pub ")
                || prefix.starts_with("pub(crate) ")
                || prefix.starts_with("pub(super) ");
            if is_fn_start {
                if let Some(body) = extract_brace_body(src, idx) {
                    return Some(body);
                }
            }
            search_from = idx + needle_plain.len();
        }
    }
    None
}

/// From byte offset `sig_start` (the start of `fn name(`), find the first
/// `{` after it and return everything up to its matching `}` (depth-counted
/// over the whole rest of the file, so nested blocks/closures/match arms
/// don't end the scan early).
fn extract_brace_body(src: &str, sig_start: usize) -> Option<String> {
    let rest = &src[sig_start..];
    let brace_rel = rest.find('{')?;
    let body_start = sig_start + brace_rel;
    let bytes = src.as_bytes();
    let mut depth: i32 = 0;
    let mut i = body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(src[body_start..=i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Extract every parameter name a function body treats as unconditionally
/// required, from the `ok_or(...)`/`ok_or_else(...)` "Missing" idiom used
/// throughout `tool_impls`/`executor`. Handles the variants actually in use:
/// `Missing '<name>'[ argument| parameter]`, `Missing required '<name>'
/// parameter`, and `Missing [required ]parameter: <name>` /
/// `Missing [required ]argument: <name>`.
fn extract_missing_param_names(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(idx) = rest.find("Missing") {
        let after = &rest[idx + "Missing".len()..];
        let after = after.strip_prefix(" required").unwrap_or(after);
        let after = after.trim_start();
        if let Some(quoted) = after.strip_prefix('\'') {
            if let Some(end) = quoted.find('\'') {
                names.push(quoted[..end].to_string());
            }
        } else if let Some(tail) = after
            .strip_prefix("parameter: ")
            .or_else(|| after.strip_prefix("argument: "))
        {
            let ident: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !ident.is_empty() {
                names.push(ident);
            }
        }
        rest = &rest[idx + "Missing".len()..];
    }
    names
}

/// Tools with no discoverable `execute_<name>` function by the naming
/// convention this test relies on (dispatched via some other path — see
/// this module's doc comment). This is a *coverage* ratchet, not a defect
/// allowlist: every name here means "not checked", not "checked and OK".
/// Shrinking it (by giving a tool a conventionally-named impl fn, or by
/// teaching this test a new lookup strategy) increases real coverage;
/// growing it should be rare and deliberate.
/// `(tool, param)` pairs where the implementation's "Missing '<param>'" check is
/// inside a mode branch, so the parameter is genuinely optional at the schema
/// level. Narrow by construction: naming the pair keeps every *other* parameter
/// of that tool under the check. Only add an entry after confirming the `ok_or`
/// really is branch-local — an unconditional requirement belongs in the schema.
const CONDITIONALLY_REQUIRED_PARAMS: &[(&str, &str)] = &[
    // dap.rs:50 — attach mode only.
    ("dap_start", "pid"),
    // dap.rs:58 — launch mode only.
    ("dap_start", "program"),
];

const NOT_CHECKED_NO_CONVENTIONAL_IMPL_FN: &[&str] = &[
    "ai_permissions",
    "ai_set_budget",
    "ai_set_mode",
    "ai_set_profile",
    "ask_user",
    "convert_buffer",
    "delegate",
    "execute_command",
    "format_buffer",
    "input_lock",
    "kb_block_member",
    "kb_unblock_member",
    "log_activity",
    "lookup_online",
    "model_exam",
    "next_error",
    "pkg_doctor",
    "pkg_sync",
    "pkg_upgrade",
    "propose_changes",
    "read_transcript",
    "run_build",
    "run_test",
    "search_tools",
    "self_test_suite",
    "shell_exec",
    "spell_check",
    "terminal_read",
    "terminal_send",
    "toggle_file_tree",
    "web_fetch",
];

/// The actual guard: every parameter an `execute_<tool>` impl treats as
/// unconditionally required must be both a declared property AND in the
/// schema's `required` list. Run `cargo test -p mae-ai
/// schema_impl_params_agree -- --nocapture` after deliberately deleting a
/// `.prop(...)`/`.required([...])` call from any tool with a checkable impl
/// to see this fail (verified manually against `lsp_workspace_symbol`
/// before landing this test — see this module's doc comment for the
/// pre-existing real bug it caught unmodified).
#[test]
fn schema_impl_params_agree() {
    let tools = ai_specific_tools(&OptionRegistry::new());
    let mut violations: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for tool in &tools {
        if NOT_CHECKED_NO_CONVENTIONAL_IMPL_FN.contains(&tool.name.as_str()) {
            continue;
        }
        let fn_name = format!("execute_{}", tool.name);
        let Some(body) = find_fn_body(&fn_name) else {
            // Not in the coverage ratchet and not found: either the
            // convention drifted (rename this into the ratchet with a
            // reason) or a genuinely new gap opened up. Fail loudly rather
            // than silently skipping — see principle #14, no unicorn-value
            // test permissiveness.
            violations.push(format!(
                "{}: no `fn {}` found in IMPL_SOURCES and not in \
                 NOT_CHECKED_NO_CONVENTIONAL_IMPL_FN — add the impl fn, or \
                 add the tool name to the ratchet with a reason",
                tool.name, fn_name
            ));
            continue;
        };
        checked += 1;
        let required_by_impl: BTreeSet<String> =
            extract_missing_param_names(&body).into_iter().collect();
        for param in &required_by_impl {
            // Conditionally-required parameters: the extraction is textual and
            // brace-depth based, so it cannot see that an `ok_or("Missing ...")`
            // sits inside a mode branch. `dap_start` needs `pid` only when
            // attaching and `program` only when launching — declaring either
            // `required` would be *wrong*, since it would force the agent to
            // send a parameter the other mode rejects. This is the documented
            // blind spot, narrowed to named (tool, param) pairs rather than
            // skipping the tool wholesale, so every other parameter of these
            // tools is still checked.
            if CONDITIONALLY_REQUIRED_PARAMS
                .iter()
                .any(|(t, p)| *t == tool.name && p == param)
            {
                continue;
            }
            if !tool.parameters.properties.contains_key(param) {
                violations.push(format!(
                    "{}: impl requires '{}' (via ok_or \"Missing ...\") but it is not a \
                     declared schema property at all — the agent cannot even see this \
                     parameter exists",
                    tool.name, param
                ));
            } else if !tool.parameters.required.iter().any(|r| r == param) {
                violations.push(format!(
                    "{}: impl requires '{}' but the schema does not list it in `required` \
                     — the agent may legitimately omit it and get a runtime error",
                    tool.name, param
                ));
            }
        }
    }

    assert!(
        checked > 100,
        "sanity check: expected the naming convention to cover >100 tools, got {checked} \
         — IMPL_SOURCES or the convention likely drifted"
    );
    assert!(
        violations.is_empty(),
        "tool schema/impl parameter disagreements ({} checked, {} violation(s)):\n{}",
        checked,
        violations.len(),
        violations.join("\n")
    );
}

/// Pins the current skip list so it can only shrink (or grow with an
/// obvious, reviewable diff) — not silently accumulate as tools are added.
/// See `NOT_CHECKED_NO_CONVENTIONAL_IMPL_FN`'s doc comment.
#[test]
fn dispatch_contract_test_coverage_is_tracked() {
    let tools = ai_specific_tools(&OptionRegistry::new());
    let tool_names: BTreeSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    for skipped in NOT_CHECKED_NO_CONVENTIONAL_IMPL_FN {
        assert!(
            tool_names.contains(skipped),
            "'{skipped}' is in the coverage-skip ratchet but is no longer a registered tool \
             — remove it from NOT_CHECKED_NO_CONVENTIONAL_IMPL_FN"
        );
    }
}
