Perform a source-level audit of the MAE codebase. Run all 4 phases below and report results with `[OK]`, `[WARN]`, or `[ERROR]` markers.

## Phase 1: Structural Health

1. Grep `crates/*/src/lib.rs` for `@stability:` markers. Report any crates missing the marker.
2. Count `pub fn`, `pub struct`, `pub enum` per crate (top 5 by count).
3. Flag any files >1500 lines in `crates/core/src/editor/`.
4. Cross-reference crate count against `Cargo.toml` workspace members.

## Phase 2: Module System Maturity

1. Parse all `modules/*/module.toml` — validate required fields (`name`, `version`, `description`, `mae_version`, `category`), semver format, mae_version constraint.
2. Check each `autoloads.scm` for `provide-feature` call and `@module` header.
3. Check whether `docs/module-template/` exists with `module.toml`, `autoloads.scm`, `README.md`.
4. Report the module dependency graph (if any modules declare dependencies).

## Phase 3: Scheme API Documentation

1. Count `register_fn` calls in `crates/scheme/src/runtime.rs`.
2. Count `scheme:*` KB nodes in `crates/core/src/kb_seed/`.
3. Cross-reference: report undocumented primitives (registered but no KB node) and orphan KB nodes (KB node but no registration).

## Phase 4: AI Provider Coverage

1. Check `crates/ai/src/context_limits.rs` TABLE for model coverage — list all model prefixes.
2. Check `ProviderHint::from_model()` — verify all model families in TABLE are covered.
3. Report verification status distribution (Verified / Testing / Untested counts).
4. Check that prompt XML files exist for all profiles x tiers. Look for: `pair-programmer.xml`, `pair-programmer-compact.xml`, `explorer.xml`, `explorer-compact.xml`, `reviewer.xml`, `reviewer-compact.xml`, `planner.xml`, `planner-compact.xml`.
5. Cross-reference models in `context_limits.rs` TABLE vs `pricing.rs` TABLE — report models with limits but no pricing (intentional for OSS models) and vice versa.

## Output Format

Group results by phase. Use:
- `[OK]` — check passed
- `[WARN]` — non-critical issue found
- `[ERROR]` — critical issue that should be fixed

End with a summary: total checks, OK count, WARN count, ERROR count.

## Optional: Runtime Checks

If MAE is running with MCP, also call:
- `audit_configuration` — runtime config health
- `self_test_suite` — get structured test plan (don't execute, just check it's available)
