#!/bin/sh
# prune-build-artifacts.sh — reclaim stale cargo build artifacts.
#
#   scripts/prune-build-artifacts.sh [-n] [-d DAYS] [-q]
#
#     -n  dry run — report what would be removed, delete nothing
#     -d  age threshold in days (default 7)
#     -q  quiet — totals only
#
# ---------------------------------------------------------------------------
# Why this exists
# ---------------------------------------------------------------------------
#
# Cargo never garbage-collects test binaries. Every rebuild whose inputs changed
# emits a NEW hash-suffixed binary into `target/<profile>/deps/` and leaves the
# previous one behind forever. Nothing in the normal workflow removes them, so a
# repo under active development accumulates one binary per test target per
# meaningful change, indefinitely.
#
# Measured on this repo (2026-08-05): 101 GB across four target directories, of
# which ~49 GB was older than a week. `daemon/target/debug/deps` alone held 1,398
# binaries over 1 MB totalling 46 GB — 858 of them stale — because MAE's daemon is
# a SEPARATE workspace (ADR-014) and therefore builds its own copy of every
# dependency, at full debuginfo, into its own target dir. Artifacts dating back
# four weeks were still present.
#
# Deleting them is safe by construction: cargo rebuilds anything it cannot find.
# The cost of over-pruning is time, never correctness.
#
# ---------------------------------------------------------------------------
# What it will and will not touch
# ---------------------------------------------------------------------------
#
# ONLY `deps/`, `incremental/` and `build/` under a directory that carries cargo's
# own `CACHEDIR.TAG` — i.e. a directory cargo itself marked as a build cache. It
# never removes a whole `target/`, never follows symlinks, and never touches
# anything outside a verified cargo target dir. A wrong `-d` costs a rebuild.
#
# Portable per CLAUDE.md principle #13: POSIX sh, no `find -printf`, no `-delete`,
# no GNU `du` flags — all of which differ or are absent on macOS.

set -eu

DAYS=7
DRY=0
QUIET=0

usage() {
    sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        -n) DRY=1; shift ;;
        -q) QUIET=1; shift ;;
        -d) [ $# -ge 2 ] || usage; DAYS="$2"; shift 2 ;;
        -h|--help) usage ;;
        *) echo "unknown argument: $1" >&2; usage ;;
    esac
done

case "$DAYS" in
    ''|*[!0-9]*) echo "error: -d expects a whole number of days, got '$DAYS'" >&2; exit 2 ;;
esac

ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

# Portable "size of these files in KiB": `du -k` is POSIX; `du -ch` totals differ
# between GNU and BSD, so sum with awk instead of trusting a `total` line.
sum_kb() {
    # reads NUL-delimited paths on stdin
    xargs -0 du -k 2>/dev/null | awk '{s+=$1} END {printf "%d", s+0}'
}

human() {
    awk -v k="$1" 'BEGIN {
        if (k > 1048576) printf "%.1f GB", k/1048576;
        else if (k > 1024) printf "%.1f MB", k/1024;
        else printf "%d KB", k;
    }'
}

total_kb=0
total_n=0

# Every cargo target dir in the repo, identified by cargo's own marker file
# rather than by a hardcoded list — so a new workspace or tool is covered the day
# it appears, with no list to keep in sync.
for tag in $(find . -name CACHEDIR.TAG -type f 2>/dev/null | sort); do
    tdir="$(dirname "$tag")"
    for profile in debug release; do
        for sub in deps incremental build; do
            dir="$tdir/$profile/$sub"
            [ -d "$dir" ] || continue

            # -mtime +N is POSIX. Restrict to regular files at depth 1 for deps
            # (the binaries); incremental/build are directories, handled below.
            if [ "$sub" = deps ]; then
                set --
                n=$(find "$dir" -maxdepth 1 -type f -mtime "+$DAYS" 2>/dev/null | wc -l | tr -d ' ')
                [ "$n" -gt 0 ] || continue
                kb=$(find "$dir" -maxdepth 1 -type f -mtime "+$DAYS" -print0 2>/dev/null | sum_kb)
            else
                n=$(find "$dir" -maxdepth 1 -mindepth 1 -mtime "+$DAYS" 2>/dev/null | wc -l | tr -d ' ')
                [ "$n" -gt 0 ] || continue
                kb=$(find "$dir" -maxdepth 1 -mindepth 1 -mtime "+$DAYS" -print0 2>/dev/null | sum_kb)
            fi

            total_kb=$((total_kb + kb))
            total_n=$((total_n + n))
            [ "$QUIET" -eq 1 ] || printf '  %-46s %5s items  %s\n' \
                "${dir#./}" "$n" "$(human "$kb")"

            if [ "$DRY" -eq 0 ]; then
                if [ "$sub" = deps ]; then
                    find "$dir" -maxdepth 1 -type f -mtime "+$DAYS" -exec rm -f {} +
                else
                    find "$dir" -maxdepth 1 -mindepth 1 -mtime "+$DAYS" -exec rm -rf {} +
                fi
            fi
        done
    done
done

if [ "$total_n" -eq 0 ]; then
    echo "No build artifacts older than ${DAYS}d."
elif [ "$DRY" -eq 1 ]; then
    echo "Would reclaim $(human "$total_kb") across $total_n items older than ${DAYS}d (dry run)."
else
    echo "Reclaimed $(human "$total_kb") across $total_n items older than ${DAYS}d."
fi
