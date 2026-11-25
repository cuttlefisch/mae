#!/bin/sh
# Run each x86_64 Linux binary given on a CPU with no extension past the
# x86-64 baseline (QEMU's `qemu64`: no SSE4, no AVX), and fail if one of them
# faults in its OWN code. Release binaries built by a toolchain that defaults
# to AVX died with SIGILL at start-up on such CPUs (v0.14.191–195, aws-lc's CPU
# detection); `--version` reaches every initialiser that runs at load time.
#
# A dynamically linked binary also loads the build host's shared libraries,
# and those are not ours to ship: the release runner's own system libraries
# fault on qemu64 before our code starts. So a fault is attributed by address.
# Inside the binary's own mapping it fails the check; outside it, the check
# says so loudly and does not fail, because nothing of the binary was shown to
# fault. Every C dependency is linked statically into the executable, so all
# of our code is inside that mapping. A statically linked binary (mae-daemon,
# mae-mcp-shim) loads nothing else and is checked completely.
#
# Usage: scripts/check-baseline-cpu.sh BINARY...
# Needs qemu-user (qemu-x86_64 or qemu-x86_64-static), binutils (nm, readelf)
# and python3. x86_64 Linux only.
set -u
# No core files in the working directory from the faults this provokes (dash
# and bash both support -c).
# shellcheck disable=SC3045
ulimit -c 0 2>/dev/null || true

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

# Prints "own <pc> <function>" when the last instruction translated before the
# fault lies inside BINARY's own mapping, else "host <pc>".
attribute() {
    python3 -I - "$1" "$2" <<'EOF'
import re, subprocess, sys
binary, log = sys.argv[1], sys.argv[2]
blocks, name = [], ""
for line in open(log, errors="replace"):
    if line.startswith("IN:"):
        name = line[3:].strip()
        continue
    m = re.match(r"0x([0-9a-f]+):", line)
    if m:
        blocks.append((name, int(m.group(1), 16)))
        name = ""
if not blocks:
    print("host ?")
    sys.exit()
pc_name, pc = blocks[-1]
headers = subprocess.run(["readelf", "-lW", binary], capture_output=True, text=True).stdout
fn = pc_name or "?"
for n, addr in reversed(blocks):
    if n:
        fn = n
        break
if "INTERP" not in headers:
    # Statically linked: nothing else is loaded, so every fault is its own.
    print(f"own {pc:#x} {fn}")
    sys.exit()
start = None
for line in subprocess.run(["nm", binary], capture_output=True, text=True).stdout.splitlines():
    parts = line.split()
    if len(parts) == 3 and parts[2] == "_start":
        start = int(parts[0], 16)
starts = [addr for n, addr in blocks if n == "_start"]
if start is None or not starts:
    # The binary's entry point was never reached: the fault came first.
    print(f"host {pc:#x}")
    sys.exit()
base = starts[0] - start
span = 0
for line in headers.splitlines():
    f = line.split()
    if f and f[0] == "LOAD":
        span = max(span, int(f[2], 16) + int(f[5], 16))
inside = base <= pc < base + span
print(f"own {pc:#x} {fn}" if inside else f"host {pc:#x}")
EOF
}

failed=0
for bin in "$@"; do
    out=$("$qemu" -cpu qemu64 "$bin" --version 2>&1)
    rc=$?
    if [ "$rc" -eq 0 ]; then
        echo "ok      $bin: $(printf '%s' "$out" | head -n 1)"
        continue
    fi
    log=$(mktemp)
    "$qemu" -cpu qemu64 -d in_asm,nochain -D "$log" "$bin" --version >/dev/null 2>&1
    where=$(attribute "$bin" "$log")
    case "$where" in
    own*)
        # 132 = 128 + SIGILL: an instruction past the baseline, in our code.
        echo "FAILED  $bin: exit $rc on -cpu qemu64, in its own code at ${where#own }" >&2
        printf '%s\n' "$out" | tail -n 3 >&2
        echo "  the last instructions translated:" >&2
        tail -n 12 "$log" | sed 's/^/    /' >&2
        failed=1
        ;;
    *)
        echo "HOST    $bin: exit $rc on -cpu qemu64, but the fault is in this host's" \
            "libraries at ${where#host }, outside the binary; not counted" >&2
        ;;
    esac
    rm -f "$log"
done
exit "$failed"
