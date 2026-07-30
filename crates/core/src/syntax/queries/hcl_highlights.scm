; Hand-authored highlights query for HCL/Terraform (Language::Terraform).
;
; @ai-caution: [architecture-debt] This is the ONE deliberate exception to
; "no local .scm query files" (every other language's query comes bundled
; from its own tree-sitter-<lang> crate, see languages.rs's module doc
; comment). tree-sitter-hcl 1.1.0 ships NO highlights query at all — neither
; in the published crate (its Cargo.toml lists `queries/*` in `include` but
; the directory doesn't exist in the tarball) nor in the upstream GitHub repo
; (verified: github.com/tree-sitter-grammars/tree-sitter-hcl has no
; `queries/` directory as of this writing). Authored here directly against
; the grammar's `node-types.json` (no `grammar.js` field/rule source is
; vendored in the published crate either, so exact child-ordering/field
; names for `block`/`attribute` could not be cross-checked against grammar
; source — only against the compiled node-types summary, cross-checked
; empirically against `tree.root_node().to_sexp()` output). If/when upstream
; ships a real highlights.scm, prefer switching to it and deleting this file
; (matches every other language's pattern) rather than maintaining a
; hand-rolled query indefinitely.
;
; @ai-caution: [rendering] Pattern ORDER matters here: `tree-sitter-highlight`
; resolves overlapping captures at the same node by LAST-PATTERN-WINS (the
; same convention `languages.rs`'s TypeScript-over-JavaScript combined query
; relies on — TS-specific captures are appended after JS's so they override).
; The general `(identifier) @variable` catch-all MUST stay first so the more
; specific `@keyword`/`@function`/`@property` patterns below it win for the
; same node — this was gotten backwards once already and caught by
; `terraform_highlights_keyword_string_number_and_comment` (a real query
; compiles fine but silently mis-highlights bug, not a compile error).

(identifier) @variable

(comment) @comment

(string_lit) @string
(template_literal) @string
(heredoc_identifier) @string.special

(numeric_lit) @number
(bool_lit) @constant.builtin
(null_lit) @constant.builtin

; The leading identifier of a block (`resource "aws_instance" "web" { ... }`
; -> "resource") is not a distinct keyword token in this grammar -- it's a
; plain `identifier`, sibling to the block's `block_start`/label
; `string_lit`s/`body`/`block_end` (per node-types.json's `block` children
; list). Capturing any direct-child identifier of a block as a keyword is a
; reasonable, conservative approximation.
(block (identifier) @keyword)

(function_call (identifier) @function)
(get_attr (identifier) @property)
