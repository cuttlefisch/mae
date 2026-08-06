#!/bin/sh
# verify.sh — Final file-on-disk verification for collab E2E tests.
#
# Checks that workspace-a, workspace-b, and shared-workspace all contain
# converged content. Run as the 'verifier' service after clients complete.

set -e

PASS=0
FAIL=0

# POSIX sh has no `local` (SC3043) — this script declares #!/bin/sh and
# CLAUDE.md principle #13 requires identical behaviour on macOS and Linux, so
# use distinctly-named globals rather than a bash-only extension.
check_file() {
    cf_path="$1"
    cf_expected="$2"
    cf_desc="$3"

    if [ ! -f "$cf_path" ]; then
        echo "FAIL: $cf_desc — file not found: $cf_path"
        FAIL=$((FAIL + 1))
        return
    fi

    if grep -q "$cf_expected" "$cf_path"; then
        echo "PASS: $cf_desc"
        PASS=$((PASS + 1))
    else
        echo "FAIL: $cf_desc — expected '$cf_expected' in $cf_path"
        echo "  actual content:"
        sed 's/^/    /' "$cf_path"
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

# Scenario 3: Per-user CRDT undo — A redid its edit, B undid its edit.
# A's final state: base + from-A (B undid from-B before A's redo)
check_file "/workspace-undo-a/undo-test.txt" "base" "Undo sharer has base content"
check_file "/workspace-undo-a/undo-test.txt" "from-A" "Undo sharer has from-A (after redo)"
# B's final state: base only (B undid its own edit, A's was already undone by A at that point)
check_file "/workspace-undo-b/undo-test.txt" "base" "Undo joiner has base content"

echo
echo "=== Results: $PASS passed, $FAIL failed ==="

if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
