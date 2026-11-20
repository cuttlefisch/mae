#!/bin/sh
# Lint only the crates a staged change touches: the pre-commit budget (seconds, not
# minutes). The whole-workspace clippy of both workspaces still runs in CI, so nothing
# is lost; it is only no longer paid on every commit.
#
# For each staged .rs or Cargo.toml file, the nearest directory whose Cargo.toml has a
# [package] section is its crate, and the nearest one (from there up) whose Cargo.toml
# has a [workspace] section is the workspace to run cargo in (the editor at the repo
# root, daemon/, or a standalone tool under tools/).
#
# Usage: scripts/clippy-changed.sh            lint what is staged
#        scripts/clippy-changed.sh --list     print "workspace<TAB>package", lint nothing
set -eu

CARGO=${CARGO:-cargo}
list_only=0
[ "${1:-}" = "--list" ] && list_only=1

has_section() { grep -q "^\[$2\]" "$1/Cargo.toml" 2>/dev/null; }

package_name() {
    awk '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^name *=/{gsub(/.*= *"|".*/, ""); print; exit}' "$1/Cargo.toml"
}

pairs=$(
    git diff --cached --name-only --diff-filter=ACMR -- '*.rs' '*Cargo.toml' |
    while IFS= read -r f; do
        d=$(dirname "$f")
        while [ "$d" != "." ] && ! has_section "$d" package; do d=$(dirname "$d"); done
        has_section "$d" package || continue
        w=$d
        while [ "$w" != "." ] && ! has_section "$w" workspace; do w=$(dirname "$w"); done
        printf '%s\t%s\n' "$w" "$(package_name "$d")"
    done | sort -u
)

if [ -z "$pairs" ]; then
    echo "clippy-changed: no Rust crate touched by the staged change"
    exit 0
fi
if [ "$list_only" = 1 ]; then
    printf '%s\n' "$pairs"
    exit 0
fi

for w in $(printf '%s\n' "$pairs" | cut -f1 | sort -u); do
    pkgs=$(printf '%s\n' "$pairs" | awk -F '\t' -v w="$w" '$1 == w { printf " -p %s", $2 }')
    feats=""
    # The editor binary's GUI code only compiles with its feature (as in `make clippy`).
    if [ "$w" = "." ] && printf '%s\n' "$pairs" | grep -q "^\.	mae$"; then feats="--features gui"; fi
    echo "clippy-changed: ($w)$pkgs $feats"
    # shellcheck disable=SC2086
    (cd "$w" && $CARGO clippy $pkgs --all-targets $feats -- -D warnings)
done
