#!/bin/sh
# verify-binary.sh — warn loudly if a RUNNING mae / mae-daemon differs from the
# freshly built target/release binary. The #1 time-sink in two-machine testing is
# verifying a fix against a stale binary (docs/collab-kb-sync-testing-lessons.md
# §4.4). Run `make build && make verify-binary && make install` before retesting.
#
# Cross-OS: resolves a process's on-disk image via /proc on Linux, `lsof` on macOS.
# Exits non-zero if any running process's image != the build (so it can gate a
# Makefile target), and also if freshness cannot be determined for a RUNNING
# process — a check that could not be performed is a failure, not a pass.
# No running process ⇒ pass (nothing to be stale).

set -u

# sha256 of a file, portable across Linux (sha256sum) + macOS (shasum -a 256).
hash_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" 2>/dev/null | awk '{print $1}'
    else
        shasum -a 256 "$1" 2>/dev/null | awk '{print $1}'
    fi
}

# Resolve the on-disk path of a running PID's executable.
exe_of() {
    pid="$1"
    if [ -r "/proc/$pid/exe" ]; then
        readlink "/proc/$pid/exe" 2>/dev/null
    elif command -v lsof >/dev/null 2>&1; then
        lsof -p "$pid" 2>/dev/null | awk '$4=="txt"{print $NF; exit}'
    fi
}

status=0

# A check that cannot be performed must FAIL, not pass quietly. This script's
# entire job is to stop you testing a stale binary, so "I couldn't tell" is the
# one outcome that must never be reported as OK.
check() {
    name="$1"
    built="$2"
    pids="$(pgrep -x "$name" 2>/dev/null || pgrep "$name" 2>/dev/null || true)"

    if [ ! -f "$built" ]; then
        if [ -n "$pids" ]; then
            # The dangerous case: something is running and there is nothing to
            # compare it against, so staleness is unknowable.
            echo "  ⚠ $name: RUNNING (pids: $pids) but no built binary at $built"
            echo "      → cannot verify freshness; run the build first"
            status=1
        else
            # Benign: not built and not running (e.g. a TUI-only checkout that
            # never builds the daemon). Nothing can be stale.
            echo "  - $name: not built, not running (nothing to be stale)"
        fi
        return
    fi

    built_hash="$(hash_of "$built")"
    if [ -z "$pids" ]; then
        echo "  - $name: not running (nothing to be stale)"
        return
    fi
    for pid in $pids; do
        exe="$(exe_of "$pid")"
        if [ -z "${exe:-}" ] || [ ! -f "$exe" ]; then
            echo "  ⚠ $name pid $pid: cannot resolve on-disk image — freshness unverifiable"
            status=1
            continue
        fi
        run_hash="$(hash_of "$exe")"
        if [ "$run_hash" = "$built_hash" ]; then
            echo "  ✓ $name pid $pid matches the fresh build"
        else
            echo "  ⚠ $name pid $pid ($exe) != fresh build ($built)"
            echo "      → run 'make install' and RESTART $name before retesting"
            status=1
        fi
    done
}

echo "verify-binary: comparing running processes to target/release builds…"
check mae        "target/release/mae"
check mae-daemon "daemon/target/release/mae-daemon"

if [ "$status" -ne 0 ]; then
    echo "FAIL: a running binary is stale — you may be testing the wrong code."
    exit 1
fi
echo "OK: no stale running binaries."
