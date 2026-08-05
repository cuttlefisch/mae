#!/bin/sh
#
# lint-shell.sh — shellcheck every shell script in the repo, at an enforced
# minimum severity.
#
# Usage:
#   scripts/lint-shell.sh [-S SEVERITY] [-l] [-h]
#
#     -S SEVERITY  minimum severity to fail on: error|warning|info|style
#                  (default: warning)
#     -l           list the files that would be checked, then exit
#     -h           this help
#
# Exit status: 0 if every script is clean at SEVERITY, 1 otherwise, 127 when
# the shellcheck binary is missing.
#
# ---------------------------------------------------------------------------
# Why `warning` and not `style`
# ---------------------------------------------------------------------------
#
# The repo had NO shell linting at all until 2026-08, across 18 scripts
# including the 832-line installer every user runs. The baseline at that point
# was 53 findings: 6 warning, 41 info, 6 style.
#
# The 6 warnings were real defects and were fixed — among them three uses of
# `local` in a `#!/bin/sh` script, which is a CLAUDE.md principle #13
# portability violation, and a variable that shellcheck could not see was
# assigned indirectly (documented with a scoped `disable` rather than silenced
# globally).
#
# `warning` is therefore an enforceable floor TODAY, with the repo already at
# zero. `info`/`style` are mostly SC2015 (`a && b || c`) and SC2059 (printf
# format from a variable) in older scripts; raising the floor to `info` is a
# worthwhile follow-up but is a cleanup, not a gate, and a gate nobody can pass
# gets disabled rather than fixed.
#
# NEW deployment scripts are held to `style` — see the deploy/ lint job.
#
# Portable per CLAUDE.md principle #13: POSIX sh, no GNU-only flags.

set -eu

SEVERITY=warning
LIST_ONLY=0

usage() {
    sed -n '3,17p' "$0" | sed 's/^#\{0,1\} \{0,1\}//'
    exit "${1:-2}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        -S) [ $# -ge 2 ] || usage; SEVERITY="$2"; shift 2 ;;
        -l) LIST_ONLY=1; shift ;;
        -h|--help) usage 0 ;;
        *) printf 'error: unknown argument: %s\n\n' "$1" >&2; usage ;;
    esac
done

case "$SEVERITY" in
    error|warning|info|style) ;;
    *) printf "error: -S expects error|warning|info|style, got '%s'\n" "$SEVERITY" >&2; exit 2 ;;
esac

ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

if ! command -v shellcheck >/dev/null 2>&1; then
    echo "error: shellcheck not found on PATH" >&2
    echo "  Fedora: sudo dnf install ShellCheck" >&2
    echo "  Debian: sudo apt install shellcheck" >&2
    echo "  macOS:  brew install shellcheck" >&2
    exit 127
fi

# Every tracked shell script. `git ls-files` rather than `find` so build output,
# vendored code and untracked scratch files are never linted — and so the set
# checked in CI is exactly the set committed.
scripts="$(git ls-files '*.sh' | sort)"

if [ -z "$scripts" ]; then
    echo "No shell scripts found." >&2
    exit 0
fi

if [ "$LIST_ONLY" -eq 1 ]; then
    printf '%s\n' "$scripts"
    exit 0
fi

count="$(printf '%s\n' "$scripts" | wc -l | tr -d ' ')"
printf 'shellcheck: %s script(s), failing at severity >= %s\n' "$count" "$SEVERITY"

failed=0
for f in $scripts; do
    if ! shellcheck -S "$SEVERITY" -f gcc "$f"; then
        failed=$((failed + 1))
    fi
done

if [ "$failed" -gt 0 ]; then
    printf '\nFAILED: %s script(s) have findings at severity >= %s\n' "$failed" "$SEVERITY" >&2
    exit 1
fi

printf 'All %s script(s) clean at severity >= %s.\n' "$count" "$SEVERITY"
