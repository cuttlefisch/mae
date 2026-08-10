#!/bin/sh
# check-test-effects.sh — run a command and fail if it changed the working tree.
#
# The test-effect sandbox (shared/effect-sandbox) is deny-by-default, but only
# for operations that were taught to ask it. That is an enumeration, and an
# enumeration leaks: `commands::tests::all_builtin_commands_dispatch` reached
# git through `Editor::git_root()`'s `current_dir()` fallback (fixed), and then
# still wrote `crates/core/export.html` through `org_export_to`'s bare relative
# path fallback (a different code path, so the guard never saw it).
#
# The reason both survived is not that the guards were weak. It is that
# NOTHING EVER LOOKED. A `git status` after the suite would have shown either
# one instantly, in CI as well as locally. This script is that look, wired in
# so it happens every time instead of when someone happens to notice.
#
# It compares before/after rather than requiring a clean tree, so a contributor
# with legitimate uncommitted work still gets a usable signal: only files the
# command itself touched are reported.
#
# Usage:
#   scripts/check-test-effects.sh cargo test --workspace
#   scripts/check-test-effects.sh make test
#
# Exit: the command's own status, unless the tree changed — then 1.

set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [args...]" >&2
    exit 2
fi

if ! command -v git >/dev/null 2>&1 || ! git rev-parse --git-dir >/dev/null 2>&1; then
    echo "check-test-effects: not a git repo (or no git) — running without the check" >&2
    exec "$@"
fi

before="$(git status --porcelain 2>/dev/null || true)"

set +e
"$@"
cmd_rc=$?
set -e

after="$(git status --porcelain 2>/dev/null || true)"

if [ "$before" = "$after" ]; then
    exit "$cmd_rc"
fi

# Report only entries the command introduced, so pre-existing local edits are
# not blamed on it. Temp files rather than `<(...)`: this is /bin/sh, and
# process substitution is a bashism (principle #13 — `make lint-shell` would
# flag it, and dash would simply fail at runtime).
tmp_before="$(mktemp)"
trap 'rm -f "$tmp_before"' EXIT INT TERM
printf '%s\n' "$before" > "$tmp_before"
new="$(printf '%s\n' "$after" | grep -vxF -f "$tmp_before" 2>/dev/null || true)"

if [ -z "$(printf '%s' "$new" | tr -d '[:space:]')" ]; then
    # Only disappearances (e.g. a build step consumed a scratch file). Not the
    # failure mode this guards, so don't fail on it — but say so.
    echo "check-test-effects: working-tree entries disappeared during the run:" >&2
    exit "$cmd_rc"
fi

cat >&2 <<EOF

==============================================================================
TEST-EFFECT LEAK: the command below modified the working tree.

  command: $*

New/changed entries:
$new

A test wrote into the repository. That is the defect class that cost a
contributor uncommitted work (see shared/effect-sandbox/src/lib.rs) and that
left \`crates/core/export.html\` regenerating on every run.

Fix the WRITE, not this check:
  * a path built from \`current_dir()\` or a bare relative name resolves against
    the crate root under \`cargo test\`;
  * if a test genuinely needs a real effect, give it a \`tempfile::tempdir()\`
    target and wrap it in \`effect_sandbox::with_external_effects\`.
==============================================================================
EOF

exit 1
