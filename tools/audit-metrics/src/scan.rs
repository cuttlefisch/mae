//! Per-file structural metrics.
//!
//! Everything here is mechanically derived from the source — no judgment, no
//! hand-maintained numbers. That is the whole point: `.claude/commands/mae-audit.md`
//! used to carry these figures as prose, and every one of them had drifted
//! (14 of 15 tracked file sizes were stale, one by +96%). Numbers that a tool
//! recomputes cannot drift.
//!
//! Length/nesting metrics use real spans (`proc-macro2`'s `span-locations`), not
//! brace counting, so a macro body or a string containing `{` cannot skew them.

use serde::{Deserialize, Serialize};
use std::path::Path;
use syn::visit::Visit;

/// Ceilings from `.claude/commands/mae-audit.md`'s "Hard Ceilings" table.
/// Kept here as the single machine-readable copy; the markdown table now
/// documents these rather than defining them.
pub const SOURCE_FILE_CEILING: usize = 800;
pub const TEST_FILE_CEILING: usize = 500;
pub const FUNCTION_CEILING: usize = 80;
pub const MATCH_ARM_CEILING: usize = 30;
pub const STRUCT_FIELD_CEILING: usize = 15;
pub const NESTING_CEILING: usize = 4;

/// Fraction of a file that may be inline tests before we flag it for
/// sibling-module extraction. `mae-audit.md` names this lever explicitly
/// ("tests are >50% of lines"), but nothing measured it until now.
pub const INLINE_TEST_DOMINANCE: f64 = 0.5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetrics {
    /// Workspace-relative, forward-slashed, stable across machines.
    pub path: String,
    pub lines: usize,
    /// Lines inside a `#[cfg(test)]` module. Split out because a 6,000-line
    /// file that is 70% inline tests needs a different remedy (extract the
    /// test module to a sibling file) than one that is 6,000 lines of logic.
    pub test_lines: usize,
    pub code_lines: usize,
    /// True for `tests/` integration crates and `*_tests.rs`/`tests.rs`
    /// siblings — these get `TEST_FILE_CEILING`, not `SOURCE_FILE_CEILING`.
    pub is_test_file: bool,
    pub max_fn_lines: usize,
    pub max_fn_name: String,
    pub max_match_arms: usize,
    pub max_struct_fields: usize,
    pub max_struct_name: String,
    pub max_nesting: usize,
    pub use_count: usize,
    pub pub_items: usize,
    pub test_count: usize,
    /// `syn` could not parse the file; length/nesting fields are 0 and should
    /// not be trusted. Surfaced rather than silently reported as clean.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parse_failed: bool,
}

impl FileMetrics {
    pub fn ceiling(&self) -> usize {
        if self.is_test_file {
            TEST_FILE_CEILING
        } else {
            SOURCE_FILE_CEILING
        }
    }

    pub fn over_ceiling(&self) -> bool {
        self.lines > self.ceiling()
    }

    /// Inline tests dominate this file — the documented remedy is extraction
    /// to a sibling `#[cfg(test)]` module file, which preserves private-item
    /// access while restoring source signal-to-noise.
    pub fn inline_tests_dominate(&self) -> bool {
        !self.is_test_file
            && self.lines > 0
            && (self.test_lines as f64) / (self.lines as f64) > INLINE_TEST_DOMINANCE
    }

    /// Every ceiling this file exceeds, as short human-readable labels.
    pub fn violations(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.over_ceiling() {
            v.push(format!("file {}>{}", self.lines, self.ceiling()));
        }
        if self.max_fn_lines > FUNCTION_CEILING {
            v.push(format!(
                "fn {}() {}>{}",
                self.max_fn_name, self.max_fn_lines, FUNCTION_CEILING
            ));
        }
        if self.max_match_arms > MATCH_ARM_CEILING {
            v.push(format!("match {}>{}", self.max_match_arms, MATCH_ARM_CEILING));
        }
        if self.max_struct_fields > STRUCT_FIELD_CEILING {
            v.push(format!(
                "struct {} {}>{}",
                self.max_struct_name, self.max_struct_fields, STRUCT_FIELD_CEILING
            ));
        }
        if self.max_nesting > NESTING_CEILING {
            v.push(format!("nesting {}>{}", self.max_nesting, NESTING_CEILING));
        }
        v
    }
}

fn is_test_path(rel: &str) -> bool {
    rel.contains("/tests/")
        || rel.ends_with("_tests.rs")
        || rel.ends_with("/tests.rs")
        || rel.starts_with("tests/")
}

/// Does this attribute list carry `#[cfg(test)]`?
fn has_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        a.parse_args::<syn::Ident>()
            .map(|i| i == "test")
            .unwrap_or(false)
    })
}

fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        let segs: Vec<String> = a
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        matches!(segs.last().map(String::as_str), Some("test"))
    })
}

#[derive(Default)]
struct Collector {
    max_fn_lines: usize,
    max_fn_name: String,
    max_match_arms: usize,
    max_struct_fields: usize,
    max_struct_name: String,
    max_nesting: usize,
    depth: usize,
    use_count: usize,
    pub_items: usize,
    test_count: usize,
    test_lines: usize,
}

impl Collector {
    fn record_fn(&mut self, name: String, block: &syn::Block, attrs: &[syn::Attribute]) {
        if has_test_attr(attrs) {
            self.test_count += 1;
        }
        let start = block.brace_token.span.open().start().line;
        let end = block.brace_token.span.close().end().line;
        let len = end.saturating_sub(start);
        if len > self.max_fn_lines {
            self.max_fn_lines = len;
            self.max_fn_name = name;
        }
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
        self.use_count += 1;
        syn::visit::visit_item_use(self, i);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        // A `#[cfg(test)] mod tests { … }` block: measure its extent, then
        // still descend (so its `#[test]` fns are counted) — but the lines
        // are attributed to `test_lines`, not to production code.
        if has_cfg_test(&i.attrs) {
            if let Some((brace, _)) = &i.content {
                let start = brace.span.open().start().line;
                let end = brace.span.close().end().line;
                self.test_lines += end.saturating_sub(start) + 1;
            }
        }
        syn::visit::visit_item_mod(self, i);
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        let n = i.fields.len();
        if n > self.max_struct_fields {
            self.max_struct_fields = n;
            self.max_struct_name = i.ident.to_string();
        }
        if matches!(i.vis, syn::Visibility::Public(_)) {
            self.pub_items += 1;
        }
        syn::visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        if matches!(i.vis, syn::Visibility::Public(_)) {
            self.pub_items += 1;
        }
        syn::visit::visit_item_enum(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        if matches!(i.vis, syn::Visibility::Public(_)) {
            self.pub_items += 1;
        }
        syn::visit::visit_item_trait(self, i);
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        if matches!(i.vis, syn::Visibility::Public(_)) {
            self.pub_items += 1;
        }
        self.record_fn(i.sig.ident.to_string(), &i.block, &i.attrs);
        syn::visit::visit_item_fn(self, i);
    }

    fn visit_impl_item_fn(&mut self, i: &'ast syn::ImplItemFn) {
        if matches!(i.vis, syn::Visibility::Public(_)) {
            self.pub_items += 1;
        }
        self.record_fn(i.sig.ident.to_string(), &i.block, &i.attrs);
        syn::visit::visit_impl_item_fn(self, i);
    }

    fn visit_expr_match(&mut self, i: &'ast syn::ExprMatch) {
        self.max_match_arms = self.max_match_arms.max(i.arms.len());
        syn::visit::visit_expr_match(self, i);
    }

    fn visit_block(&mut self, i: &'ast syn::Block) {
        self.depth += 1;
        self.max_nesting = self.max_nesting.max(self.depth);
        syn::visit::visit_block(self, i);
        self.depth -= 1;
    }
}

/// Compute every metric for one file. Never panics: an unparseable file is
/// reported with `parse_failed` set rather than silently counted as clean.
pub fn measure(rel_path: &str, source: &str) -> FileMetrics {
    let lines = source.lines().count();
    let is_test_file = is_test_path(rel_path);

    let mut m = FileMetrics {
        path: rel_path.to_string(),
        lines,
        test_lines: 0,
        code_lines: lines,
        is_test_file,
        max_fn_lines: 0,
        max_fn_name: String::new(),
        max_match_arms: 0,
        max_struct_fields: 0,
        max_struct_name: String::new(),
        max_nesting: 0,
        use_count: 0,
        pub_items: 0,
        test_count: 0,
        parse_failed: false,
    };

    let Ok(ast) = syn::parse_file(source) else {
        m.parse_failed = true;
        return m;
    };

    let mut c = Collector::default();
    c.visit_file(&ast);

    // A whole test-file's lines are test lines; the `#[cfg(test)]` walk above
    // only finds *inline* modules.
    m.test_lines = if is_test_file { lines } else { c.test_lines.min(lines) };
    m.code_lines = lines.saturating_sub(m.test_lines);
    m.max_fn_lines = c.max_fn_lines;
    m.max_fn_name = c.max_fn_name;
    m.max_match_arms = c.max_match_arms;
    m.max_struct_fields = c.max_struct_fields;
    m.max_struct_name = c.max_struct_name;
    // `visit_block` counts the fn body itself as depth 1; the ceiling is about
    // nesting *within* a function, so subtract that outermost level.
    m.max_nesting = c.max_nesting.saturating_sub(1);
    m.use_count = c.use_count;
    m.pub_items = c.pub_items;
    m.test_count = c.test_count;
    m
}

/// Every `.rs` file in the repo, excluding build artifacts. Deliberately walks
/// `daemon/` and `tools/` too: both are real code, and the daemon is a whole
/// second workspace the previous audit prose under-covered.
pub fn collect_files(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `target` anywhere (root, daemon/, tools/*/), plus VCS/node noise.
            if name == "target" || name == ".git" || name == "node_modules" {
                continue;
            }
            walk(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if let Ok(src) = std::fs::read_to_string(&path) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, src));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_fn_length_from_real_spans_not_brace_counting() {
        // The `{` inside the string literal must not inflate the count.
        let src = "fn f() {\n    let s = \"{\";\n    println!(\"{}\", s);\n}\n";
        let m = measure("crates/x/src/a.rs", src);
        assert_eq!(m.max_fn_lines, 3, "span-based length, got {m:?}");
        assert_eq!(m.max_fn_name, "f");
    }

    #[test]
    fn inline_cfg_test_module_lines_are_not_counted_as_code() {
        let src = "fn a() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let m = measure("crates/x/src/a.rs", src);
        assert!(m.test_lines > 0, "expected inline test lines: {m:?}");
        assert_eq!(m.code_lines, m.lines - m.test_lines);
        assert_eq!(m.test_count, 1);
    }

    #[test]
    fn test_files_use_the_lower_ceiling() {
        let src = "fn a() {}\n";
        assert_eq!(measure("crates/x/src/a.rs", src).ceiling(), SOURCE_FILE_CEILING);
        assert_eq!(measure("crates/x/tests/a.rs", src).ceiling(), TEST_FILE_CEILING);
        assert_eq!(measure("crates/x/src/a_tests.rs", src).ceiling(), TEST_FILE_CEILING);
    }

    #[test]
    fn nesting_excludes_the_function_body_itself() {
        // One `if` inside a fn body is nesting depth 1, not 2.
        let m = measure("a.rs", "fn f() {\n  if x {\n    g();\n  }\n}\n");
        assert_eq!(m.max_nesting, 1, "{m:?}");
    }

    #[test]
    fn unparseable_file_is_flagged_not_silently_clean() {
        let m = measure("a.rs", "fn ( this is not rust @@@");
        assert!(m.parse_failed, "must not report a broken file as clean");
        assert!(m.lines > 0, "line count still works without parsing");
    }

    #[test]
    fn struct_field_and_match_arm_maxima_are_recorded_with_names() {
        let src = "pub struct S { pub a: u8, pub b: u8 }\nfn f(x: u8) { match x { 1 => {}, _ => {} } }\n";
        let m = measure("a.rs", src);
        assert_eq!(m.max_struct_fields, 2);
        assert_eq!(m.max_struct_name, "S");
        assert_eq!(m.max_match_arms, 2);
    }
}
