## Summary

<!-- What does this PR do? 1-3 sentences. -->

## Test Plan

<!-- How was this tested? -->

- [ ] `make ci` passes
- [ ] No new warnings from `cargo clippy`
- [ ] New features have tests
- [ ] No file exceeds 3,000 lines

## Version Bump

<!--
On merge, the version-bump workflow reads Cargo.toml's current version and
bumps it based on PR labels or commit messages:

  Label (takes precedence)     Commit prefix         Result
  ─────────────────────────    ──────────────────    ────────
  release:patch                fix(...):             0.8.0 → 0.8.1
  release:minor                feat(...):            0.8.0 → 0.9.0
  release:major                feat(...)!:           0.8.0 → 1.0.0

Rules:
1. Do NOT manually change the version in Cargo.toml or VERSION — the
   workflow handles it on merge.
2. Add a `release:patch` label for bug fixes and docs-only changes.
3. If your PR has `feat(...)` commits but should NOT bump minor, add a
   `release:patch` label to override.
4. If no label is set, the workflow scans commit messages:
   feat → minor, everything else → patch.
-->

- [ ] Version label set (or commit prefixes are correct for auto-detection)
