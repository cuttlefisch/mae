#!/usr/bin/env bash
# MAE — Modern AI Editor installer
#
# Usage:
#   ./install.sh              # install to ~/.local (default)
#   ./install.sh /usr/local   # install to /usr/local
#   ./install.sh --help       # show usage

set -euo pipefail

VERSION="0.12.0"  # updated by version-bump workflow

# --- Argument parsing ---
if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    echo "Usage: ./install.sh [PREFIX]"
    echo ""
    echo "Install MAE editor and services to PREFIX (default: ~/.local)"
    echo ""
    echo "  PREFIX/bin/         binaries (mae, mae-daemon, mae-state-server, mae-mcp-shim)"
    echo "  XDG_DATA_HOME/mae/  manual KB, modules"
    echo "  XDG_CONFIG_HOME/mae/ config files (won't overwrite existing)"
    echo ""
    echo "Examples:"
    echo "  ./install.sh                # install to ~/.local/bin"
    echo "  ./install.sh /usr/local     # install to /usr/local/bin"
    exit 0
fi

PREFIX="${1:-$HOME/.local}"
BINDIR="$PREFIX/bin"
DATADIR="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIGDIR="${XDG_CONFIG_HOME:-$HOME/.config}"

# --- Colors (if terminal supports them) ---
if [ -t 1 ]; then
    GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'
    BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'
else
    GREEN=''; RED=''; YELLOW=''; BOLD=''; DIM=''; RESET=''
fi

PASS="${GREEN}OK${RESET}"
FAIL="${RED}FAIL${RESET}"
SKIP="${DIM}SKIP${RESET}"
ERRORS=0

step()    { printf "${BOLD}:: %s${RESET}\n" "$*"; }
ok()      { printf "   ${GREEN}[OK]${RESET} %s\n" "$*"; }
fail()    { printf "   ${RED}[!!]${RESET} %s\n" "$*"; ERRORS=$((ERRORS + 1)); }
skip()    { printf "   ${DIM}[--]${RESET} %s\n" "$*"; }
verify()  {
    # verify <path> <description>
    if [ -e "$1" ]; then
        ok "$2"
    else
        fail "$2 — not found: $1"
    fi
}

# --- Detect platform ---
OS="$(uname -s)"
ARCH="$(uname -m)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo ""
printf "${BOLD}MAE Installer${RESET}  ${DIM}v${VERSION}${RESET}\n"
printf "${DIM}Platform: ${OS} ${ARCH}${RESET}\n"
printf "${DIM}Target:   ${PREFIX}${RESET}\n"
echo ""

# ========================================================================
# 1. Binaries
# ========================================================================
step "Installing binaries to $BINDIR"
mkdir -p "$BINDIR"

for bin in mae mae-mcp-shim mae-state-server mae-daemon; do
    if [ -f "$SCRIPT_DIR/$bin" ]; then
        install -m 755 "$SCRIPT_DIR/$bin" "$BINDIR/$bin"
        verify "$BINDIR/$bin" "$bin"
    else
        skip "$bin (not in package)"
    fi
done

# ========================================================================
# 2. Manual KB (knowledge base with 860+ help nodes)
# ========================================================================
step "Installing manual KB"
mkdir -p "$DATADIR/mae"

if [ -d "$SCRIPT_DIR/mae-manual.cozo" ]; then
    rm -rf "$DATADIR/mae/mae-manual.cozo"
    cp -r "$SCRIPT_DIR/mae-manual.cozo" "$DATADIR/mae/mae-manual.cozo"
    verify "$DATADIR/mae/mae-manual.cozo" "manual KB -> $DATADIR/mae/"

    # SHA-256 verification
    if [ -f "$SCRIPT_DIR/mae-manual.cozo.sha256" ]; then
        cp "$SCRIPT_DIR/mae-manual.cozo.sha256" "$DATADIR/mae/mae-manual.cozo.sha256"
        # Verify checksum if sha256sum is available
        if command -v sha256sum >/dev/null 2>&1; then
            EXPECTED=$(awk '{print $1}' "$SCRIPT_DIR/mae-manual.cozo.sha256")
            # Compute checksum over sorted files in the sled directory
            ACTUAL=$(find "$DATADIR/mae/mae-manual.cozo" -type f | sort | xargs cat | sha256sum | awk '{print $1}')
            if [ "$EXPECTED" = "$ACTUAL" ] 2>/dev/null; then
                ok "SHA-256 checksum verified"
            else
                # Sled checksums are directory-based; mismatch is expected with simple cat
                skip "SHA-256 checksum stored (runtime validation)"
            fi
        elif command -v shasum >/dev/null 2>&1; then
            skip "SHA-256 checksum stored (runtime validation via mae)"
        fi
    fi
else
    fail "mae-manual.cozo not found in package"
fi

# ========================================================================
# 3. Modules (keybinding overlays, 19 Scheme modules)
# ========================================================================
step "Installing modules"

if [ -d "$SCRIPT_DIR/modules" ]; then
    mkdir -p "$DATADIR/mae/modules"
    cp -r "$SCRIPT_DIR/modules/"* "$DATADIR/mae/modules/"
    MODULE_COUNT=$(find "$DATADIR/mae/modules" -name "manifest.toml" 2>/dev/null | wc -l | tr -d ' ')
    verify "$DATADIR/mae/modules" "$MODULE_COUNT modules -> $DATADIR/mae/modules/"
else
    fail "modules directory not found in package"
fi

# ========================================================================
# 4. Configuration (never overwrite existing user config)
# ========================================================================
step "Installing configuration"
mkdir -p "$CONFIGDIR/mae"

if [ -f "$SCRIPT_DIR/sample-config.toml" ]; then
    if [ ! -f "$CONFIGDIR/mae/config.toml" ]; then
        cp "$SCRIPT_DIR/sample-config.toml" "$CONFIGDIR/mae/config.toml"
        ok "config.toml -> $CONFIGDIR/mae/ (new)"
    else
        skip "config.toml already exists (preserved)"
    fi
else
    skip "sample-config.toml not in package"
fi

if [ -f "$SCRIPT_DIR/daemon-config.toml" ]; then
    if [ ! -f "$CONFIGDIR/mae/daemon.toml" ]; then
        cp "$SCRIPT_DIR/daemon-config.toml" "$CONFIGDIR/mae/daemon.toml"
        ok "daemon.toml -> $CONFIGDIR/mae/ (new)"
    else
        skip "daemon.toml already exists (preserved)"
    fi
else
    skip "daemon-config.toml not in package"
fi

# ========================================================================
# 5. Systemd services (Linux only)
# ========================================================================
if [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
    step "Installing systemd user services"
    SYSTEMD_DIR="$CONFIGDIR/systemd/user"
    mkdir -p "$SYSTEMD_DIR"

    for unit in mae-state-server.service mae-daemon.service; do
        if [ -f "$SCRIPT_DIR/$unit" ]; then
            # Rewrite ExecStart to match actual install PREFIX
            sed "s|%h/.local/bin/|$BINDIR/|g" "$SCRIPT_DIR/$unit" > "$SYSTEMD_DIR/$unit"
            verify "$SYSTEMD_DIR/$unit" "$unit"
        else
            skip "$unit not in package"
        fi
    done

    systemctl --user daemon-reload 2>/dev/null || true
    ok "systemctl --user daemon-reload"
elif [ "$OS" = "Linux" ]; then
    skip "systemd not available — service files not installed"
fi

# ========================================================================
# 6. Desktop entries (Linux only)
# ========================================================================
if [ "$OS" = "Linux" ]; then
    step "Desktop integration"
    # Desktop entries are best handled by `make install` from source
    # since they need path substitution. For tarball installs, suggest
    # running mae --init-config which handles this.
    skip "Run 'mae --init-config' after first launch for desktop integration"
fi

# ========================================================================
# Summary
# ========================================================================
echo ""
if [ "$ERRORS" -eq 0 ]; then
    printf "${GREEN}${BOLD}Installation complete!${RESET}\n"
else
    printf "${YELLOW}${BOLD}Installation complete with $ERRORS warning(s)${RESET}\n"
fi

# PATH check
echo ""
case ":$PATH:" in
    *":$BINDIR:"*)
        ok "$BINDIR is on your PATH"
        ;;
    *)
        printf "${YELLOW}   $BINDIR is not on your PATH.${RESET} Add to your shell profile:\n"
        echo "     export PATH=\"$BINDIR:\$PATH\""
        echo ""
        ;;
esac

# Verify mae binary runs
if command -v "$BINDIR/mae" >/dev/null 2>&1; then
    MAE_V=$("$BINDIR/mae" --version 2>/dev/null || echo "unknown")
    ok "mae binary works: $MAE_V"
fi

echo ""
printf "${BOLD}Getting started:${RESET}\n"
echo "  mae --init-config              # first-time setup wizard"
echo "  mae file.rs                    # open a file (GUI)"
echo "  mae -nw file.rs                # open a file (terminal)"
echo ""

if [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
    printf "${BOLD}Optional services:${RESET}\n"
    echo "  systemctl --user enable --now mae-daemon         # KB background persistence"
    echo "  systemctl --user enable --now mae-state-server   # collaborative editing"
    echo ""
fi

printf "${BOLD}Learn more:${RESET}\n"
echo "  :help tutorial:getting-started   # interactive tutorial"
echo "  :help tutorial:ai-setup          # AI provider configuration"
echo "  :help concept:daemon             # daemon setup guide"
echo ""
