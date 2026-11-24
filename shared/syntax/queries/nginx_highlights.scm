; tree-sitter-nginx 1.0.1's highlights query, vendored verbatim from its
; package (queries/highlights.scm): the crate ships the file but exports no
; HIGHLIGHTS_QUERY constant (its bindings/rust/lib.rs has it commented out),
; so it cannot be reached as the other grammars' queries are. Replace it with
; the constant if a later release exports one, and re-vendor it on a version
; bump: a query out of step with its grammar fails to compile
; (every_grammar_builds_a_configuration).
;
; MIT License, Copyright (c) 2024 Vladimir Levin
; (https://github.com/opa-oz/tree-sitter-nginx; full text in its LICENSE).

(comment) @comment @spell

(value) @variable

(attribute (keyword) @attribute)

[
  (location_modifier)
  "="
] @operator

[
  (keyword)
  "location"
] @keyword

[
  "if"
  "map"
] @keyword.conditional

(directive (keyword) @constant)

(boolean) @boolean

[
  (auto)
  (constant)
  (level)
  (connection_method)
  (var)
  condition: (condition)
] @variable.builtin

[
  (string_literal)
  (quoted_string_literal)
  (file)
  (mask)
] @string

(directive (variable) @variable.parameter)

(directive (variable (keyword) @variable.parameter))

(location_route) @string.special
";" @punctuation.delimiter

[
  (numeric_literal)
  (time)
  (size)
  (cpumask)
] @number

[
  "{"
  "}"
] @punctuation.bracket
