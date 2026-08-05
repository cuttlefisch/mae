#!/usr/bin/env bash
#
# run-tests.sh — prove the mae_daemon role's preflight checks REFUSE the
# configurations they exist to refuse.
#
# Usage:
#   run-tests.sh [-v] [-k PATTERN] [-l LOGFILE] [-h]
#
#     -v          verbose: show each ansible-playbook run's full output
#     -k PATTERN  run only cases whose name contains PATTERN
#     -l LOGFILE  write the full transcript here (default: ./preflight-tests.log)
#     -h          this help
#
# Exit status: 0 if every case behaved as specified, 1 otherwise.
#
# ---------------------------------------------------------------------------
# Why this exists
# ---------------------------------------------------------------------------
#
# A deployment role's safety checks are the one part nobody exercises in normal
# use: they only run on the day someone makes the mistake. A check that silently
# stopped matching — a renamed variable, a filter that now returns a string
# instead of a list, an `assert` whose `that:` is always truthy — looks
# identical to a check that works, because the happy path passes either way.
#
# So every case here is a NEGATIVE case: a config that must be rejected, paired
# with the message it must be rejected WITH. A case that fails for the wrong
# reason is reported as a failure, not a pass — otherwise a syntax error
# anywhere in preflight would make every test "pass".
#
# One positive control runs first. Without it, a preflight that rejects
# everything unconditionally would score 100%.
#
# Portable per CLAUDE.md principle #13: bash (ansible requires it anyway), no
# GNU-only flags, no `mktemp` templates that differ on macOS.

set -euo pipefail

VERBOSE=0
FILTER=""
LOGFILE="./preflight-tests.log"

usage() {
    sed -n '2,17p' "$0" | sed 's/^#\{0,1\} \{0,1\}//'
    exit "${1:-2}"
}

while getopts ':vk:l:h' opt; do
    case "$opt" in
        v) VERBOSE=1 ;;
        k) FILTER="$OPTARG" ;;
        l) LOGFILE="$OPTARG" ;;
        h) usage 0 ;;
        :) printf 'error: -%s requires an argument\n\n' "$OPTARG" >&2; usage ;;
        ?) printf 'error: unknown option -%s\n\n' "$OPTARG" >&2; usage ;;
    esac
done
shift $((OPTIND - 1))

HERE="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
cd "$HERE"

command -v ansible-playbook >/dev/null 2>&1 || {
    echo "error: ansible-playbook not found on PATH" >&2
    exit 127
}

# Resolve the role from the parent directory rather than relying on the cwd:
# this script runs from tests/, and ansible.cfg's roles_path is relative.
export ANSIBLE_ROLES_PATH="$HERE/../roles"
# Deterministic, quiet output — this script parses it.
export ANSIBLE_PYTHON_INTERPRETER=auto_silent
export ANSIBLE_DEPRECATION_WARNINGS=False
export ANSIBLE_LOCALHOST_WARNING=False
export ANSIBLE_STDOUT_CALLBACK=default
export ANSIBLE_NOCOLOR=1
# Never inherit an operator's ansible.cfg (log_path, callbacks, become) into a
# test run; the results must not depend on whose machine this is. An empty file
# rather than /dev/null: ansible rejects a config path with no .cfg extension.
ANSIBLE_CONFIG="$(mktemp "${TMPDIR:-/tmp}/mae-empty-XXXXXX").cfg"
export ANSIBLE_CONFIG

: > "$LOGFILE"

log()  { printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$LOGFILE"; }
say()  { printf '%s\n' "$*"; log "$*"; }

PASS=0
FAIL=0
SKIP=0

# WORK holds one generated inventory per case. Removed on every exit path,
# including a failure or a Ctrl-C, so a interrupted run leaves nothing behind.
WORK="$(mktemp -d "${TMPDIR:-/tmp}/mae-preflight-XXXXXX")"
cleanup() { rm -rf "$WORK" "$ANSIBLE_CONFIG"; }
trap cleanup EXIT INT TERM

# run_case NAME EXPECT PATTERN <<'YAML' ... YAML
#
#   EXPECT   accept | refuse
#   PATTERN  a string that must appear in the output (for `refuse`, the reason;
#            ignored for `accept`)
#
# Reading the config from stdin keeps each case's inventory adjacent to the
# assertion about it, rather than in a separate file the reader has to go find.
run_case() {
    name="$1"; expect="$2"; pattern="${3:-}"
    if [ -n "$FILTER" ] && [ "${name#*"$FILTER"}" = "$name" ]; then
        SKIP=$((SKIP + 1))
        return 0
    fi

    inv="$WORK/$(printf '%s' "$name" | tr ' /' '__').yml"
    cat > "$inv"

    log "=== CASE: $name (expect: $expect) ==="
    set +e
    out="$(ansible-playbook -i "$inv" preflight.yml 2>&1)"
    rc=$?
    set -e
    printf '%s\n' "$out" >> "$LOGFILE"
    [ "$VERBOSE" -eq 1 ] && printf '%s\n' "$out"

    case "$expect" in
        accept)
            if [ "$rc" -eq 0 ]; then
                say "  PASS  $name (accepted, as specified)"
                PASS=$((PASS + 1))
            else
                say "  FAIL  $name — a VALID config was rejected (rc=$rc)"
                say "        This is the positive control. If it fails, every"
                say "        'refuse' result below is meaningless."
                printf '%s\n' "$out" | tail -20 | sed 's/^/        /'
                FAIL=$((FAIL + 1))
            fi
            ;;
        refuse)
            if [ "$rc" -eq 0 ]; then
                say "  FAIL  $name — preflight ACCEPTED a config it must refuse"
                FAIL=$((FAIL + 1))
            elif printf '%s' "$out" | grep -qF "$pattern"; then
                say "  PASS  $name (refused, with the right reason)"
                PASS=$((PASS + 1))
            else
                # Refused, but not for the reason under test — e.g. a typo in
                # the harness, or an earlier check firing first. Counting this
                # as a pass is how a broken preflight scores 100%.
                say "  FAIL  $name — refused, but not for the expected reason"
                say "        expected to find: $pattern"
                printf '%s\n' "$out" | grep -iE 'fatal|assertion' | head -5 | sed 's/^/        /'
                FAIL=$((FAIL + 1))
            fi
            ;;
        *)
            say "  FAIL  $name — bad EXPECT '$expect' in the test itself"
            FAIL=$((FAIL + 1))
            ;;
    esac
}

say "mae_daemon preflight safety checks"
say "log: $LOGFILE"
say ""

# --- Positive control ------------------------------------------------------
# Must come first. A preflight that refuses everything would otherwise pass
# every negative case below and look perfect.
run_case "valid config is accepted" accept "" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: staging
          collab_bind: "127.0.0.1:9473"
          collab_auth_mode: key
        - name: prod
          collab_bind: "127.0.0.1:9475"
          collab_auth_mode: key
YAML

# --- Reproducibility -------------------------------------------------------
run_case "unpinned version is refused" refuse "must be an explicit x.y.z release" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: ""
      mae_daemon_instances:
        - name: staging
          collab_bind: "127.0.0.1:9473"
YAML

run_case "a non-semver version is refused" refuse "must be an explicit x.y.z release" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "latest"
      mae_daemon_instances:
        - name: staging
          collab_bind: "127.0.0.1:9473"
YAML

# --- Nothing to deploy -----------------------------------------------------
run_case "no instances is refused" refuse "would install a binary and start" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances: []
YAML

# --- Instance naming, including path traversal ------------------------------
run_case "path traversal in an instance name is refused" refuse "is not usable" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: "../../etc/cron.d/evil"
          collab_bind: "127.0.0.1:9473"
YAML

run_case "an absolute path as an instance name is refused" refuse "is not usable" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: "/etc/shadow"
          collab_bind: "127.0.0.1:9473"
YAML

run_case "an over-long instance name is refused" refuse "is not usable" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
          collab_bind: "127.0.0.1:9473"
YAML

run_case "duplicate instance names are refused" refuse "Duplicate instance names" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: prod
          collab_bind: "127.0.0.1:9473"
        - name: prod
          collab_bind: "127.0.0.1:9475"
YAML

# --- Resource collisions ---------------------------------------------------
run_case "two instances on one collab port are refused" refuse "share a collab bind address" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: staging
          collab_bind: "127.0.0.1:9473"
        - name: prod
          collab_bind: "127.0.0.1:9473"
YAML

# --- The security check ----------------------------------------------------
# mae-daemon's own auth default is "none". Binding that to a routable address
# is an open, unauthenticated sync server.
run_case "unauthenticated bind to all interfaces is refused" refuse "Set collab_auth_mode: key" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: prod
          collab_bind: "0.0.0.0:9473"
          collab_auth_mode: none
YAML

run_case "plaintext psk on a routable address is refused" refuse "Set collab_auth_mode: key" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: prod
          collab_bind: "10.0.0.5:9473"
          collab_auth_mode: psk
YAML

run_case "unauthenticated bind to loopback is allowed" accept "" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_instances:
        - name: dev
          collab_bind: "127.0.0.1:9473"
          collab_auth_mode: none
YAML

# --- Supply chain ----------------------------------------------------------
run_case "disabling checksum verification alone is refused" refuse "needs both" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_verify_checksum: false
      mae_daemon_instances:
        - name: prod
          collab_bind: "127.0.0.1:9473"
          collab_auth_mode: key
YAML

run_case "disabling verification with the explicit override is allowed" accept "" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_verify_checksum: false
      mae_daemon_allow_unverified: true
      mae_daemon_instances:
        - name: prod
          collab_bind: "127.0.0.1:9473"
          collab_auth_mode: key
YAML

# --- Rollback capability ---------------------------------------------------
run_case "zero retained releases is refused" refuse "would prune the release" <<'YAML'
all:
  hosts:
    localhost:
      mae_daemon_version: "0.14.93"
      mae_daemon_keep_releases: 0
      mae_daemon_instances:
        - name: prod
          collab_bind: "127.0.0.1:9473"
          collab_auth_mode: key
YAML

say ""
say "----------------------------------------"
say "passed: $PASS   failed: $FAIL   skipped: $SKIP"
if [ "$FAIL" -gt 0 ]; then
    say "FAILED — see $LOGFILE for the full transcript"
    exit 1
fi
say "All preflight safety checks behaved as specified."
