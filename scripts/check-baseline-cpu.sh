#!/bin/sh
# Run each x86_64 Linux binary given on a CPU with no extension past the
# x86-64 baseline (QEMU's `qemu64`: no SSE4, no AVX), and fail if any of them
# dies. Release binaries built by a toolchain that defaults to AVX died with
# SIGILL at start-up on such CPUs (v0.14.191–193, aws-lc's CPU detection);
# `--version` reaches every library initialiser that runs at load time.
#
# Usage: scripts/check-baseline-cpu.sh BINARY...
# Needs qemu-user (qemu-x86_64 or qemu-x86_64-static). x86_64 Linux only.
set -u

qemu=""
for q in qemu-x86_64-static qemu-x86_64; do
    if command -v "$q" >/dev/null 2>&1; then qemu=$q; break; fi
done
if [ -z "$qemu" ]; then
    echo "check-baseline-cpu: qemu-user not found (qemu-x86_64[-static])" >&2
    exit 2
fi
if [ "$#" -eq 0 ]; then
    echo "usage: $0 BINARY..." >&2
    exit 2
fi

failed=0
for bin in "$@"; do
    out=$("$qemu" -cpu qemu64 "$bin" --version 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "ok      $bin: $(printf '%s' "$out" | head -n 1)"
    else
        # 132 = 128 + SIGILL: an instruction past the baseline.
        echo "FAILED  $bin: exit $rc on -cpu qemu64" >&2
        printf '%s\n' "$out" | tail -n 3 >&2
        # Name the code that faulted: the last translated blocks before the
        # signal, by symbol (release binaries are not stripped).
        log=$(mktemp)
        "$qemu" -cpu qemu64 -d in_asm,nochain -D "$log" "$bin" --version >/dev/null 2>&1
        echo "  last functions entered before the fault:" >&2
        grep '^IN: ' "$log" | uniq | tail -n 6 | sed 's/^/    /' >&2
        rm -f "$log"
        failed=1
    fi
done
exit "$failed"
