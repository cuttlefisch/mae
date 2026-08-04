#!/bin/sh
# check-heavy-e2e-lists.sh — keep the "heavy subprocess e2e" list in sync.
#
# Some test binaries spawn a real `mae --headless` subprocess and wait for it to
# bind a socket. They must not run concurrently with the other ~8000 tests, or
# they lose the CPU race and fail with:
#
#     headless instance never bound its stable socket at ...
#
# That list is duplicated in THREE places, by necessity — nextest's test-group
# only serializes the group against ITSELF, so the workflows additionally split
# the run into "heavy" and "not heavy" invocations:
#
#   1. .config/nextest.toml   — the heavy-subprocess-e2e test-group filter
#   2. .github/workflows/ci.yml       — HEAVY_FILTER
#   3. .github/workflows/badges.yml   — HEAVY_FILTER
#
# Three copies of one fact drift, and did: headless_guard_leak_regression_e2e
# arrived with #613 and was added to none of them. CI didn't catch it (it runs
# --release, where the same test takes 3.1s instead of 26.9s and comfortably
# beats the 30s budget); the badges job, which ran debug, failed twice.
#
# This script is the guard. A test file is "heavy" iff it references
# `headless_test_support` — the helper that does the spawn-and-wait. That rule
# currently selects exactly the 8 intended binaries.
#
# Portable per CLAUDE.md principle #13: POSIX sh, no GNU-only flags, works the
# same on macOS and Linux.

set -eu

ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

NEXTEST=".config/nextest.toml"
CI=".github/workflows/ci.yml"
BADGES=".github/workflows/badges.yml"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The binaries that ACTUALLY spawn a headless instance.
grep -l 'headless_test_support' crates/mae/tests/*.rs 2>/dev/null \
  | sed 's|.*/||; s|\.rs$||' | sort -u > "$tmp/actual"

# What each of the three files declares.
#
# NB the character class MUST include digits: every one of these names ends in
# `e2e`, and a `[a-z_]*` class silently matches NOTHING (it stops before the
# `2`, so there is no closing paren). A grep that quietly returns empty would
# make this whole check pass vacuously — the exact failure mode it exists to
# prevent.
extract() {
    grep -o 'binary([a-z0-9_]*)' "$1" 2>/dev/null \
      | sed 's|^binary(||; s|)$||' | sort -u
}

extract "$NEXTEST" > "$tmp/nextest"
extract "$CI"      > "$tmp/ci"
extract "$BADGES"  > "$tmp/badges"

# A vacuous pass is a failure. If any extraction came back empty, the pattern
# or the file layout changed and this guard is no longer guarding anything.
for f in actual nextest ci badges; do
    if [ ! -s "$tmp/$f" ]; then
        echo "FAIL: extracted an EMPTY list for '$f' — this check cannot verify anything." >&2
        echo "      The file layout or the grep pattern changed. Fix this script." >&2
        exit 1
    fi
done

status=0

report_missing() {
    label="$1"; file="$2"; listfile="$3"
    missing="$(comm -23 "$tmp/actual" "$listfile")"
    if [ -n "$missing" ]; then
        status=1
        echo "FAIL: $file is missing heavy test binaries:" >&2
        echo "$missing" | sed 's|^|        |' >&2
    fi
    extra="$(comm -13 "$tmp/actual" "$listfile")"
    if [ -n "$extra" ]; then
        status=1
        echo "FAIL: $file lists binaries that do not use headless_test_support:" >&2
        echo "$extra" | sed 's|^|        |' >&2
        echo "      Either they no longer spawn a headless instance (drop them)," >&2
        echo "      or they spawn one another way (teach this script the signature)." >&2
    fi
    [ "$label" = "" ] && return 0 || return 0
}

report_missing "nextest" "$NEXTEST" "$tmp/nextest"
report_missing "ci"      "$CI"      "$tmp/ci"
report_missing "badges"  "$BADGES"  "$tmp/badges"

if [ "$status" -ne 0 ]; then
    cat >&2 <<EOF

A test binary that spawns a real \`mae --headless\` instance must be listed in
ALL THREE of:

    $NEXTEST   (heavy-subprocess-e2e test-group filter)
    $CI        (HEAVY_FILTER)
    $BADGES    (HEAVY_FILTER)

Add it as \`+ binary(<name>)\` to each. Leaving it out does not fail loudly —
it fails as an intermittent 30s socket-bind timeout, usually in a different
test, which is why this check exists.
EOF
    exit 1
fi

echo "heavy-e2e lists agree: $(wc -l < "$tmp/actual" | tr -d ' ') binaries in all three."
