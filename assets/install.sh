#!/usr/bin/env bash
# MAE — Modern AI Editor installer
#
# Usage:
#   ./install.sh                    # install/upgrade to ~/.local (default)
#   ./install.sh /usr/local         # install/upgrade to /usr/local
#   ./install.sh --uninstall        # remove MAE from ~/.local
#   ./install.sh --uninstall /opt   # remove MAE from /opt
#   ./install.sh --help             # show usage

set -euo pipefail

VERSION="0.13.1"  # updated by version-bump workflow

BINARIES="mae mae-mcp-shim mae-state-server mae-daemon"
SERVICES="mae-state-server mae-daemon"

# ========================================================================
# Argument parsing
# ========================================================================
ACTION="install"
PREFIX=""

for arg in "$@"; do
    case "$arg" in
        --help|-h)
            echo "Usage: ./install.sh [--uninstall] [PREFIX]"
            echo ""
            echo "Install, upgrade, or uninstall MAE."
            echo ""
            echo "  PREFIX defaults to ~/.local"
            echo ""
            echo "  ./install.sh                    # fresh install or upgrade"
            echo "  ./install.sh /usr/local         # install to /usr/local"
            echo "  ./install.sh --uninstall        # remove from ~/.local"
            echo "  ./install.sh --uninstall /opt   # remove from /opt"
            echo ""
            echo "Install locations:"
            echo "  PREFIX/bin/              binaries"
            echo "  XDG_DATA_HOME/mae/       manual KB, modules"
            echo "  XDG_CONFIG_HOME/mae/     config files (preserved on upgrade/uninstall)"
            echo "  ~/.config/systemd/user/  systemd units (Linux)"
            exit 0
            ;;
        --uninstall)
            ACTION="uninstall"
            ;;
        *)
            PREFIX="$arg"
            ;;
    esac
done

PREFIX="${PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
DATADIR="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIGDIR="${XDG_CONFIG_HOME:-$HOME/.config}"

# ========================================================================
# Colors and helpers
# ========================================================================
if [ -t 1 ]; then
    GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'
    BOLD='\033[1m'; DIM='\033[2m'; RESET='\033[0m'
else
    GREEN=''; RED=''; YELLOW=''; BOLD=''; DIM=''; RESET=''
fi

ERRORS=0

step()    { printf "\n${BOLD}:: %s${RESET}\n" "$*"; }
ok()      { printf "   ${GREEN}[OK]${RESET} %s\n" "$*"; }
fail()    { printf "   ${RED}[!!]${RESET} %s\n" "$*"; ERRORS=$((ERRORS + 1)); }
skip()    { printf "   ${DIM}[--]${RESET} %s\n" "$*"; }
warn()    { printf "   ${YELLOW}[??]${RESET} %s\n" "$*"; }
verify()  { if [ -e "$1" ]; then ok "$2"; else fail "$2 — not found: $1"; fi; }

OS="$(uname -s)"
ARCH="$(uname -m)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ========================================================================
# Stop running services before modifying binaries
# ========================================================================
stop_services() {
    if [ "$OS" != "Linux" ] || ! command -v systemctl >/dev/null 2>&1; then
        return
    fi
    for svc in $SERVICES; do
        if systemctl --user is-active "$svc" >/dev/null 2>&1; then
            systemctl --user stop "$svc" 2>/dev/null || true
            ok "stopped $svc"
            # Record that we stopped it so we can restart later
            eval "STOPPED_${svc//-/_}=1"
        fi
    done
}

# Restart services that were running before we stopped them
restart_services() {
    if [ "$OS" != "Linux" ] || ! command -v systemctl >/dev/null 2>&1; then
        return
    fi
    for svc in $SERVICES; do
        varname="STOPPED_${svc//-/_}"
        if [ "${!varname:-0}" = "1" ]; then
            if systemctl --user start "$svc" 2>/dev/null; then
                ok "restarted $svc"
            else
                warn "failed to restart $svc — start manually: systemctl --user start $svc"
            fi
        fi
    done
}

# ========================================================================
# UNINSTALL
# ========================================================================
if [ "$ACTION" = "uninstall" ]; then
    echo ""
    printf "${BOLD}MAE Uninstaller${RESET}  ${DIM}v${VERSION}${RESET}\n"
    printf "${DIM}Removing from: ${PREFIX}${RESET}\n"

    # --- Stop services ---
    step "Stopping services"
    if [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
        for svc in $SERVICES; do
            if systemctl --user is-active "$svc" >/dev/null 2>&1; then
                systemctl --user stop "$svc" 2>/dev/null || true
                ok "stopped $svc"
            else
                skip "$svc not running"
            fi
            if systemctl --user is-enabled "$svc" >/dev/null 2>&1; then
                systemctl --user disable "$svc" 2>/dev/null || true
                ok "disabled $svc"
            fi
        done
    else
        skip "systemd not available"
    fi

    # --- Remove binaries ---
    step "Removing binaries from $BINDIR"
    for bin in $BINARIES; do
        if [ -f "$BINDIR/$bin" ]; then
            rm -f "$BINDIR/$bin"
            ok "removed $bin"
        else
            skip "$bin not installed"
        fi
    done

    # --- Remove data (KB + modules, NOT user KBs) ---
    step "Removing shared data"
    if [ -d "$DATADIR/mae/mae-manual.cozo" ]; then
        rm -rf "$DATADIR/mae/mae-manual.cozo"
        rm -f "$DATADIR/mae/mae-manual.cozo.sha256"
        ok "removed manual KB"
    else
        skip "manual KB not found"
    fi
    if [ -d "$DATADIR/mae/modules" ]; then
        rm -rf "$DATADIR/mae/modules"
        ok "removed modules"
    else
        skip "modules not found"
    fi

    # --- Remove systemd units ---
    step "Removing systemd units"
    SYSTEMD_DIR="$CONFIGDIR/systemd/user"
    for unit in mae-state-server.service mae-daemon.service; do
        if [ -f "$SYSTEMD_DIR/$unit" ]; then
            rm -f "$SYSTEMD_DIR/$unit"
            ok "removed $unit"
        else
            skip "$unit not installed"
        fi
    done
    if [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload 2>/dev/null || true
    fi

    # --- Preserve user config ---
    step "User data (preserved)"
    if [ -d "$CONFIGDIR/mae" ]; then
        skip "config dir preserved: $CONFIGDIR/mae/"
        skip "  (remove manually if desired: rm -rf $CONFIGDIR/mae)"
    fi
    if [ -d "$DATADIR/mae" ]; then
        # Check if anything remains (user KBs, transcripts, etc.)
        REMAINING=$(find "$DATADIR/mae" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')
        if [ "$REMAINING" -gt 0 ]; then
            skip "data dir has $REMAINING remaining items: $DATADIR/mae/"
            skip "  (user KBs, transcripts, etc. — remove manually if desired)"
        else
            rm -rf "$DATADIR/mae"
            ok "removed empty data dir"
        fi
    fi

    echo ""
    printf "${GREEN}${BOLD}Uninstall complete.${RESET}\n"
    echo ""
    exit 0
fi

# ========================================================================
# INSTALL / UPGRADE
# ========================================================================
echo ""
printf "${BOLD}MAE Installer${RESET}  ${DIM}v${VERSION}${RESET}\n"
printf "${DIM}Platform: ${OS} ${ARCH}${RESET}\n"
printf "${DIM}Target:   ${PREFIX}${RESET}\n"

# --- Detect existing installation ---
UPGRADE=0
OLD_VERSION=""
if [ -x "$BINDIR/mae" ]; then
    OLD_VERSION=$("$BINDIR/mae" --version 2>/dev/null || echo "unknown")
    UPGRADE=1
fi

if [ "$UPGRADE" -eq 1 ]; then
    step "Upgrading existing installation"
    ok "current version: $OLD_VERSION"
    ok "new version:     $VERSION"

    # Stop running services before replacing binaries
    stop_services

    # Back up existing binaries
    for bin in $BINARIES; do
        if [ -f "$BINDIR/$bin" ]; then
            cp "$BINDIR/$bin" "$BINDIR/$bin.bak"
        fi
    done
    ok "backed up existing binaries (.bak)"
fi

# ========================================================================
# 1. Binaries
# ========================================================================
step "Installing binaries to $BINDIR"
mkdir -p "$BINDIR"

for bin in $BINARIES; do
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

    # SHA-256 checksum
    if [ -f "$SCRIPT_DIR/mae-manual.cozo.sha256" ]; then
        cp "$SCRIPT_DIR/mae-manual.cozo.sha256" "$DATADIR/mae/mae-manual.cozo.sha256"
        ok "SHA-256 checksum stored (validated at runtime)"
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
    skip "Run 'mae --init-config' after first launch for desktop integration"
fi

# ========================================================================
# Post-install: restart services that were running before upgrade
# ========================================================================
if [ "$UPGRADE" -eq 1 ]; then
    step "Restarting services"
    restart_services

    # Clean up backups on success
    for bin in $BINARIES; do
        rm -f "$BINDIR/$bin.bak"
    done
    ok "cleaned up backup files"
fi

# ========================================================================
# Verification
# ========================================================================
step "Verifying installation"

# PATH check
case ":$PATH:" in
    *":$BINDIR:"*)
        ok "$BINDIR is on PATH"
        ;;
    *)
        warn "$BINDIR is not on your PATH — add to your shell profile:"
        echo "       export PATH=\"$BINDIR:\$PATH\""
        ;;
esac

# Verify mae binary runs
if [ -x "$BINDIR/mae" ]; then
    MAE_V=$("$BINDIR/mae" --version 2>/dev/null || echo "")
    if [ -n "$MAE_V" ]; then
        ok "mae runs: $MAE_V"
    else
        fail "mae binary exists but --version failed"
    fi
else
    fail "mae binary not found at $BINDIR/mae"
fi

# ========================================================================
# Summary
# ========================================================================
echo ""
if [ "$ERRORS" -eq 0 ]; then
    if [ "$UPGRADE" -eq 1 ]; then
        printf "${GREEN}${BOLD}Upgrade complete!${RESET} ${DIM}($OLD_VERSION -> $VERSION)${RESET}\n"
    else
        printf "${GREEN}${BOLD}Installation complete!${RESET}\n"
    fi
else
    printf "${YELLOW}${BOLD}Completed with $ERRORS warning(s)${RESET}\n"
fi

echo ""
if [ "$UPGRADE" -eq 0 ]; then
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
else
    printf "${BOLD}Manage:${RESET}\n"
    echo "  ./install.sh --uninstall         # remove MAE"
fi
echo ""
