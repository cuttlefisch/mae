#!/bin/sh
# verify.sh — Final file-on-disk verification for collab E2E tests.
#
# Checks that workspace-a, workspace-b, and shared-workspace all contain
# converged content. Run as the 'verifier' service after clients complete.

set -e

PASS=0
FAIL=0

check_file() {
    local path="$1"
    local expected="$2"
    local desc="$3"

    if [ ! -f "$path" ]; then
        echo "FAIL: $desc — file not found: $path"
        FAIL=$((FAIL + 1))
        return
    fi

    if grep -q "$expected" "$path"; then
        echo "PASS: $desc"
        PASS=$((PASS + 1))
    else
        echo "FAIL: $desc — expected '$expected' in $path"
        echo "  actual content:"
        cat "$path" | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

echo "=== Collab E2E File Verification ==="
echo

# Scenario 1: Separate filesystems — Share → Join → Edit → :saveas
check_file "/workspace-a/test.txt" "Hello from Client A" "Client A file has Client A content"
check_file "/workspace-a/test.txt" "Hello from Client B" "Client A file has Client B content (via CRDT)"
check_file "/workspace-b/test.txt" "Hello from Client A" "Client B file has Client A content (via join)"
check_file "/workspace-b/test.txt" "Hello from Client B" "Client B file has Client B content"

# Scenario 2: Shared filesystem — both clients wrote to the same path.
# Content should be identical due to CRDT convergence.
check_file "/shared-workspace/test.txt" "Hello from Client A" "Shared disk has Client A content"
check_file "/shared-workspace/test.txt" "Hello from Client B" "Shared disk has Client B content"

echo
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
