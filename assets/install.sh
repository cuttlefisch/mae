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

VERSION="0.14.115"  # updated by version-bump workflow

BINARIES="mae mae-mcp-shim mae-daemon"
SERVICES="mae-daemon"
LAUNCHD_LABEL="com.cuttlefisch.mae-daemon"
# mae-headless (ADR-055) is deliberately NOT in $SERVICES/stop_services/
# restart_services: unlike mae-daemon (one singleton service), it's a
# systemd TEMPLATE unit (mae-headless@<project-hash>.service) instantiated
# per-project, on demand -- there's no single "mae-headless" unit to
# stop/restart across an upgrade. Its template file is placed (and removed
# on uninstall) below, same as the daemon's, but never auto-enabled/started
# -- a user opts a specific project instance in themselves
# (`systemctl --user enable --now mae-headless@<hash>`) when they want a
# persistent headless instance instead of the extension/CLI spawning one
# on demand.
LAUNCHD_LABEL_HEADLESS="com.cuttlefisch.mae-headless"

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
            echo "  PREFIX/bin/                      binaries"
            echo "  XDG_DATA_HOME/mae/               modules, your KBs (the ADR KB too, if installed)"
            echo "  XDG_CONFIG_HOME/mae/              config files (preserved on upgrade/uninstall)"
            echo "  ~/.config/systemd/user/           systemd units (Linux)"
            echo "  ~/Library/LaunchAgents/           launchd agents (macOS)"
            echo "  ~/Applications/ or /Applications/ .app bundle (macOS)"
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
WARNINGS=0

step()    { printf "\n${BOLD}:: %s${RESET}\n" "$*"; }
ok()      { printf "   ${GREEN}[OK]${RESET} %s\n" "$*"; }
fail()    { printf "   ${RED}[!!]${RESET} %s\n" "$*"; ERRORS=$((ERRORS + 1)); }
skip()    { printf "   ${DIM}[--]${RESET} %s\n" "$*"; }
warn()    { printf "   ${YELLOW}[??]${RESET} %s\n" "$*"; WARNINGS=$((WARNINGS + 1)); }
verify()  { if [ -e "$1" ]; then ok "$2"; else fail "$2 — not found: $1"; fi; }

# Verify a file is executable
verify_exec() {
    if [ -x "$1" ]; then
        ok "$2"
    elif [ -f "$1" ]; then
        fail "$2 — exists but not executable: $1"
    else
        fail "$2 — not found: $1"
    fi
}

OS="$(uname -s)"
ARCH="$(uname -m)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# macOS .app install location
if [ "$OS" = "Darwin" ]; then
    if [ -w "/Applications" ]; then
        APP_DIR="/Applications"
    else
        APP_DIR="$HOME/Applications"
    fi
    LAUNCHD_DIR="$HOME/Library/LaunchAgents"
    LOGDIR="$HOME/Library/Logs/mae"
fi

# ========================================================================
# Stop running services before modifying binaries
# ========================================================================
stop_services() {
    if [ "$OS" = "Darwin" ]; then
        if launchctl list "$LAUNCHD_LABEL" >/dev/null 2>&1; then
            launchctl unload "$LAUNCHD_DIR/$LAUNCHD_LABEL.plist" 2>/dev/null || true
            ok "stopped $LAUNCHD_LABEL (launchd)"
            STOPPED_LAUNCHD=1
        fi
        return
    fi

    if [ "$OS" != "Linux" ] || ! command -v systemctl >/dev/null 2>&1; then
        return
    fi
    for svc in $SERVICES; do
        if systemctl --user is-active "$svc" >/dev/null 2>&1; then
            systemctl --user stop "$svc" 2>/dev/null || true
            ok "stopped $svc"
            eval "STOPPED_${svc//-/_}=1"
        fi
    done
}

# Restart services that were running before we stopped them
restart_services() {
    if [ "$OS" = "Darwin" ]; then
        if [ "${STOPPED_LAUNCHD:-0}" = "1" ]; then
            if launchctl load "$LAUNCHD_DIR/$LAUNCHD_LABEL.plist" 2>/dev/null; then
                ok "restarted $LAUNCHD_LABEL (launchd)"
            else
                warn "failed to restart $LAUNCHD_LABEL — load manually: launchctl load $LAUNCHD_DIR/$LAUNCHD_LABEL.plist"
            fi
        fi
        return
    fi

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
    if [ "$OS" = "Darwin" ]; then
        if launchctl list "$LAUNCHD_LABEL" >/dev/null 2>&1; then
            launchctl unload "$LAUNCHD_DIR/$LAUNCHD_LABEL.plist" 2>/dev/null || true
            ok "stopped $LAUNCHD_LABEL"
        else
            skip "$LAUNCHD_LABEL not running"
        fi
    elif [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
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
        skip "no service manager available"
    fi

    # --- Remove binaries ---
    step "Removing binaries from $BINDIR"
    for bin in $BINARIES; do
        if [ -f "$BINDIR/$bin" ]; then
            rm -f "$BINDIR/$bin"
            if [ ! -f "$BINDIR/$bin" ]; then
                ok "removed $bin"
            else
                fail "failed to remove $bin"
            fi
        else
            skip "$bin not installed"
        fi
    done

    # --- Remove .app bundle (macOS) ---
    if [ "$OS" = "Darwin" ]; then
        step "Removing .app bundle"
        for dir in "$HOME/Applications" "/Applications"; do
            if [ -d "$dir/MAE.app" ]; then
                rm -rf "$dir/MAE.app"
                if [ ! -d "$dir/MAE.app" ]; then
                    ok "removed $dir/MAE.app"
                else
                    fail "failed to remove $dir/MAE.app"
                fi
            fi
        done
    fi

    # --- Remove data (KB + modules, NOT user KBs) ---
    step "Removing shared data"
    if [ -d "$DATADIR/mae/mae-manual.cozo" ]; then
        rm -rf "$DATADIR/mae/mae-manual.cozo"
        rm -f "$DATADIR/mae/mae-manual.cozo.sha256"
        ok "removed manual KB"
    else
        skip "manual KB not found"
    fi
    if [ -d "$DATADIR/mae/mae-devpractices.cozo" ]; then
        rm -rf "$DATADIR/mae/mae-devpractices.cozo"
        rm -f "$DATADIR/mae/mae-devpractices.cozo.sha256"
        ok "removed DevPractices KB"
    else
        skip "DevPractices KB not found"
    fi
    if [ -d "$DATADIR/mae/mae-practices.cozo" ]; then
        rm -rf "$DATADIR/mae/mae-practices.cozo"
        rm -f "$DATADIR/mae/mae-practices.cozo.sha256"
        ok "removed MaePractices KB"
    else
        skip "MaePractices KB not found"
    fi
    # `-e`, not `-d`: sqlite stores are FILES (ADR-108). A store installed
    # before that change is still a directory, so uninstall must accept both.
    if [ -e "$DATADIR/mae/mae-adr.cozo" ]; then
        rm -rf "$DATADIR/mae/mae-adr.cozo"
        rm -f "$DATADIR/mae/mae-adr.cozo.sha256"
        ok "removed ADR KB"
    else
        skip "ADR KB not found"
    fi
    if [ -d "$DATADIR/mae/modules" ]; then
        rm -rf "$DATADIR/mae/modules"
        ok "removed modules"
    else
        skip "modules not found"
    fi

    # --- Remove service units ---
    step "Removing service configuration"
    if [ "$OS" = "Darwin" ]; then
        if [ -f "$LAUNCHD_DIR/$LAUNCHD_LABEL.plist" ]; then
            rm -f "$LAUNCHD_DIR/$LAUNCHD_LABEL.plist"
            ok "removed $LAUNCHD_LABEL.plist"
        else
            skip "launchd agent not installed"
        fi
        if [ -f "$LAUNCHD_DIR/$LAUNCHD_LABEL_HEADLESS.plist" ]; then
            rm -f "$LAUNCHD_DIR/$LAUNCHD_LABEL_HEADLESS.plist"
            ok "removed $LAUNCHD_LABEL_HEADLESS.plist"
        else
            skip "headless launchd agent template not installed"
        fi
    elif [ "$OS" = "Linux" ]; then
        SYSTEMD_DIR="$CONFIGDIR/systemd/user"
        if [ -f "$SYSTEMD_DIR/mae-daemon.service" ]; then
            rm -f "$SYSTEMD_DIR/mae-daemon.service"
            ok "removed mae-daemon.service"
        else
            skip "mae-daemon.service not installed"
        fi
        if [ -f "$SYSTEMD_DIR/mae-headless@.service" ]; then
            rm -f "$SYSTEMD_DIR/mae-headless@.service"
            ok "removed mae-headless@.service"
        else
            skip "mae-headless@.service not installed"
        fi
        if command -v systemctl >/dev/null 2>&1; then
            systemctl --user daemon-reload 2>/dev/null || true
        fi
    fi

    # --- Preserve user config ---
    step "User data (preserved)"
    if [ -d "$CONFIGDIR/mae" ]; then
        skip "config dir preserved: $CONFIGDIR/mae/"
        skip "  (remove manually if desired: rm -rf $CONFIGDIR/mae)"
    fi
    if [ -d "$DATADIR/mae" ]; then
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
    if [ "$ERRORS" -eq 0 ]; then
        printf "${GREEN}${BOLD}Uninstall complete.${RESET}\n"
    else
        printf "${YELLOW}${BOLD}Uninstall completed with $ERRORS error(s).${RESET}\n"
    fi
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

INSTALLED_BINS=0
for bin in $BINARIES; do
    if [ -f "$SCRIPT_DIR/$bin" ]; then
        install -m 755 "$SCRIPT_DIR/$bin" "$BINDIR/$bin"
        verify_exec "$BINDIR/$bin" "$bin"
        INSTALLED_BINS=$((INSTALLED_BINS + 1))
    else
        skip "$bin (not in package)"
    fi
done

if [ "$INSTALLED_BINS" -eq 0 ]; then
    fail "no binaries found in package — is this a valid MAE distribution?"
fi

# Clear quarantine on CLI binaries (macOS — unsigned binaries get quarantined)
if [ "$OS" = "Darwin" ] && command -v xattr >/dev/null 2>&1; then
    for bin in $BINARIES; do
        if [ -f "$BINDIR/$bin" ]; then
            xattr -cr "$BINDIR/$bin" 2>/dev/null || true
        fi
    done
    ok "cleared quarantine on CLI binaries"
fi

# ========================================================================
# 2. macOS .app bundle
# ========================================================================
if [ "$OS" = "Darwin" ] && [ -d "$SCRIPT_DIR/MAE.app" ]; then
    step "Installing MAE.app to $APP_DIR"
    mkdir -p "$APP_DIR"

    # Remove old .app if present
    if [ -d "$APP_DIR/MAE.app" ]; then
        rm -rf "$APP_DIR/MAE.app"
        ok "removed previous MAE.app"
    fi

    cp -R "$SCRIPT_DIR/MAE.app" "$APP_DIR/MAE.app"
    verify "$APP_DIR/MAE.app/Contents/MacOS/mae" "MAE.app binary"
    verify "$APP_DIR/MAE.app/Contents/Info.plist" "MAE.app Info.plist"

    # Clear quarantine attribute (unsigned app)
    if command -v xattr >/dev/null 2>&1; then
        xattr -cr "$APP_DIR/MAE.app" 2>/dev/null || true
        ok "cleared quarantine flag (xattr -cr)"
    fi
elif [ "$OS" = "Darwin" ]; then
    skip "MAE.app not in package (TUI-only install)"
fi

# ========================================================================
# 3. System KB corpora — nothing to install
#
# The manual, MaePractices and DevPractices corpora are compiled into the
# `mae` binary (crates/mae/src/system_corpus.rs) and built into stores on
# first run, under the XDG cache dir. They are no longer shipped as
# pre-built `.cozo` directories, which is why this step only *removes*
# things.
#
# Why the change: a pre-built store was 53-159x its source text, was sled
# (which the sqlite-only daemon cannot open at all), was rewritten by cozo
# on first open so it could never actually be checksum-verified after
# installation, and was absent entirely on Windows, in the Docker image and
# under `cargo install`. Embedding the sources makes the corpora present by
# construction on every platform, with no packaging step to forget.
#
# Upgrade path: an older install left stores here. They are dead weight now
# — MAE reads the cache-built ones — so remove them rather than leaving
# hundreds of MB orphaned with nothing pointing at them. Only the exact
# names MAE itself shipped are touched; anything else in the data dir is the
# user's and is left alone.
# ========================================================================
step "Removing superseded pre-built KB stores"
mkdir -p "$DATADIR/mae"

removed_any=0
for kb in mae-manual mae-practices mae-devpractices; do
    if [ -d "$DATADIR/mae/$kb.cozo" ]; then
        rm -rf "$DATADIR/mae/$kb.cozo"
        rm -f "$DATADIR/mae/$kb.cozo.sha256"
        ok "removed superseded $kb.cozo (now built from the embedded corpus)"
        removed_any=1
    fi
done

# MAE's own migration debris, which this block used to leave behind: cozo
# renames a store aside as `<name>.cozo.sled.bak-<timestamp>` when migrating
# sled -> sqlite, and matching only `.cozo`/`.cozo.sha256` orphaned every one of
# them. They accumulate per migration — 198 MB across 31 directories on one
# developer machine — and then made the end-of-uninstall summary claim the data
# dir was non-empty because of "user KBs, transcripts, etc.", which was false.
#
# Scoped to MAE's OWN store names, never a glob over the data dir, so a user KB
# that happens to have been migrated is never touched.
for kb in mae-manual mae-practices mae-devpractices mae-adr; do
    for bak in "$DATADIR/mae/$kb.cozo.sled.bak-"*; do
        [ -e "$bak" ] || continue
        rm -rf "$bak"
        ok "removed orphaned $(basename "$bak")"
        removed_any=1
    done
done

if [ "$removed_any" -eq 0 ]; then
    skip "no pre-built KB stores to remove"
fi

# ========================================================================
# 3c. ADR KB (MAE's own architecture decisions, ADR-059 — deliberately
#     opt-in, not auto-registered; bundled so it's available to kb_register)
# ========================================================================
step "Installing ADR KB"

# `-e`, not `-d` -- see the note in the uninstall path above.
if [ -e "$SCRIPT_DIR/mae-adr.cozo" ]; then
    rm -rf "$DATADIR/mae/mae-adr.cozo"
    cp -r "$SCRIPT_DIR/mae-adr.cozo" "$DATADIR/mae/mae-adr.cozo"
    verify "$DATADIR/mae/mae-adr.cozo" "ADR KB -> $DATADIR/mae/"

    if [ -f "$SCRIPT_DIR/mae-adr.cozo.sha256" ]; then
        cp "$SCRIPT_DIR/mae-adr.cozo.sha256" "$DATADIR/mae/mae-adr.cozo.sha256"
        # Stored for a manual `shasum -c` against the published asset only.
        # It deliberately does NOT say "validated at runtime", as this line
        # used to: MAE never checked it, and could not have. cozo rewrites a
        # sled store the first time it is opened, so any checksum taken at
        # packaging time stops matching the moment the store is used.
        ok "SHA-256 checksum stored (for manual verification against the published asset)"
    fi

    if [ "$OS" = "Darwin" ] && command -v xattr >/dev/null 2>&1; then
        xattr -cr "$DATADIR/mae/mae-adr.cozo" 2>/dev/null || true
        ok "cleared quarantine on ADR KB"
    fi
else
    # `skip`, not `warn`: release packages deliberately do not carry the ADR KB
    # (it ships as its own asset), so this is the normal case for every user and
    # a warning here would be noise on every single install.
    skip "ADR KB not in package (opt-in — 'make fetch-adr-kb', then kb_register it)"
fi

# ========================================================================
# 4. Modules (keybinding overlays, 19 Scheme modules)
# ========================================================================
step "Installing modules"

if [ -d "$SCRIPT_DIR/modules" ]; then
    mkdir -p "$DATADIR/mae/modules"
    cp -r "$SCRIPT_DIR/modules/"* "$DATADIR/mae/modules/"
    MODULE_COUNT=$(find "$DATADIR/mae/modules" -name "module.toml" 2>/dev/null | wc -l | tr -d ' ')
    if [ "$MODULE_COUNT" -ge 1 ]; then
        ok "$MODULE_COUNT modules -> $DATADIR/mae/modules/"
    else
        fail "modules copied but no module.toml found"
    fi

    # Clear quarantine on modules (macOS)
    if [ "$OS" = "Darwin" ] && command -v xattr >/dev/null 2>&1; then
        xattr -cr "$DATADIR/mae/modules" 2>/dev/null || true
        ok "cleared quarantine on modules"
    fi
else
    fail "modules directory not found in package"
fi

# ========================================================================
# 5. Configuration (never overwrite existing user config)
# ========================================================================
step "Installing configuration"
mkdir -p "$CONFIGDIR/mae"

if [ -f "$SCRIPT_DIR/sample-config.toml" ]; then
    if [ ! -f "$CONFIGDIR/mae/config.toml" ]; then
        cp "$SCRIPT_DIR/sample-config.toml" "$CONFIGDIR/mae/config.toml"
        verify "$CONFIGDIR/mae/config.toml" "config.toml (new)"
    else
        skip "config.toml already exists (preserved)"
    fi
else
    skip "sample-config.toml not in package"
fi

if [ -f "$SCRIPT_DIR/daemon-config.toml" ]; then
    if [ ! -f "$CONFIGDIR/mae/daemon.toml" ]; then
        cp "$SCRIPT_DIR/daemon-config.toml" "$CONFIGDIR/mae/daemon.toml"
        verify "$CONFIGDIR/mae/daemon.toml" "daemon.toml (new)"
    else
        skip "daemon.toml already exists (preserved)"
    fi
else
    skip "daemon-config.toml not in package"
fi

# The MULTI-instance template (staging + production, or process-per-tenant via
# mae-daemon@.service). Copied as a template to edit, NOT as a live config —
# it names no instance and would be wrong for every one of them.
if [ -f "$SCRIPT_DIR/daemon-instance-config.toml" ]; then
    cp "$SCRIPT_DIR/daemon-instance-config.toml" "$CONFIGDIR/mae/daemon-instance-config.toml"
    verify "$CONFIGDIR/mae/daemon-instance-config.toml" "daemon-instance-config.toml (template)"
else
    skip "daemon-instance-config.toml not in package"
fi

# ========================================================================
# 6. Service management (systemd on Linux, launchd on macOS)
# ========================================================================
if [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
    step "Installing systemd user services"
    SYSTEMD_DIR="$CONFIGDIR/systemd/user"
    mkdir -p "$SYSTEMD_DIR"

    if [ -f "$SCRIPT_DIR/mae-daemon.service" ]; then
        # Rewrite ExecStart to match actual install PREFIX
        sed "s|%h/.local/bin/|$BINDIR/|g" "$SCRIPT_DIR/mae-daemon.service" > "$SYSTEMD_DIR/mae-daemon.service"
        verify "$SYSTEMD_DIR/mae-daemon.service" "mae-daemon.service"
    else
        skip "mae-daemon.service not in package"
    fi

    if [ -f "$SCRIPT_DIR/mae-daemon@.service" ]; then
        # The per-instance template unit (ADR-060 Phase E). Installed but never
        # enabled: each instantiation needs its own daemon-<name>.toml first
        # (see daemon-instance-config.toml), so enabling it here would start an
        # instance with no config.
        sed "s|%h/.local/bin/|$BINDIR/|g" "$SCRIPT_DIR/mae-daemon@.service" > "$SYSTEMD_DIR/mae-daemon@.service"
        verify "$SYSTEMD_DIR/mae-daemon@.service" "mae-daemon@.service"
    else
        skip "mae-daemon@.service not in package"
    fi

    if [ -f "$SCRIPT_DIR/mae-headless@.service" ]; then
        # Same rewrite; never enabled/started here -- see the LAUNCHD_LABEL_HEADLESS
        # comment above for why (templated, per-project, opt-in unit).
        sed "s|%h/.local/bin/|$BINDIR/|g" "$SCRIPT_DIR/mae-headless@.service" > "$SYSTEMD_DIR/mae-headless@.service"
        verify "$SYSTEMD_DIR/mae-headless@.service" "mae-headless@.service"
    else
        skip "mae-headless@.service not in package"
    fi

    systemctl --user daemon-reload 2>/dev/null || true
    ok "systemctl --user daemon-reload"
elif [ "$OS" = "Linux" ]; then
    skip "systemd not available — service files not installed"
elif [ "$OS" = "Darwin" ]; then
    step "Installing launchd agent"
    mkdir -p "$LAUNCHD_DIR"
    mkdir -p "$LOGDIR"

    PLIST_SRC="$SCRIPT_DIR/$LAUNCHD_LABEL.plist"
    PLIST_DST="$LAUNCHD_DIR/$LAUNCHD_LABEL.plist"

    if [ -f "$PLIST_SRC" ]; then
        # Rewrite paths in plist template
        sed -e "s|__BINDIR__|$BINDIR|g" \
            -e "s|__LOGDIR__|$LOGDIR|g" \
            "$PLIST_SRC" > "$PLIST_DST"
        verify "$PLIST_DST" "launchd agent ($LAUNCHD_LABEL)"

        # Validate plist syntax
        if command -v plutil >/dev/null 2>&1; then
            if plutil -lint "$PLIST_DST" >/dev/null 2>&1; then
                ok "plist syntax valid"
            else
                fail "plist syntax invalid — launchd won't load it"
            fi
        fi
    else
        skip "launchd plist not in package"
    fi

    HEADLESS_PLIST_SRC="$SCRIPT_DIR/$LAUNCHD_LABEL_HEADLESS.plist"
    HEADLESS_PLIST_DST="$LAUNCHD_DIR/$LAUNCHD_LABEL_HEADLESS.plist"

    if [ -f "$HEADLESS_PLIST_SRC" ]; then
        # Same rewrite; never loaded here -- templated/opt-in, see the
        # LAUNCHD_LABEL_HEADLESS comment above.
        sed -e "s|__BINDIR__|$BINDIR|g" \
            -e "s|__LOGDIR__|$LOGDIR|g" \
            "$HEADLESS_PLIST_SRC" > "$HEADLESS_PLIST_DST"
        verify "$HEADLESS_PLIST_DST" "launchd agent template ($LAUNCHD_LABEL_HEADLESS)"

        if command -v plutil >/dev/null 2>&1; then
            if plutil -lint "$HEADLESS_PLIST_DST" >/dev/null 2>&1; then
                ok "plist syntax valid"
            else
                fail "plist syntax invalid — launchd won't load it"
            fi
        fi
    else
        skip "headless launchd plist not in package"
    fi
fi

# ========================================================================
# 7. Desktop integration hints
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
        warn "$BINDIR is not on your PATH"
        if [ "$OS" = "Darwin" ]; then
            SHELL_NAME="$(basename "${SHELL:-/bin/zsh}")"
            case "$SHELL_NAME" in
                zsh)
                    echo "       Add to ~/.zprofile (or ~/.zshrc):"
                    echo "         export PATH=\"$BINDIR:\$PATH\""
                    ;;
                bash)
                    echo "       Add to ~/.bash_profile:"
                    echo "         export PATH=\"$BINDIR:\$PATH\""
                    ;;
                *)
                    echo "       Add to your shell profile:"
                    echo "         export PATH=\"$BINDIR:\$PATH\""
                    ;;
            esac
        else
            echo "       Add to your shell profile:"
            echo "         export PATH=\"$BINDIR:\$PATH\""
        fi
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

# Verify supporting binaries
for bin in mae-mcp-shim mae-daemon; do
    if [ -x "$BINDIR/$bin" ]; then
        ok "$bin is executable"
    elif [ -f "$BINDIR/$bin" ]; then
        fail "$bin exists but is not executable"
    else
        warn "$bin not installed (optional)"
    fi
done

# Verify data files
#
# The manual, DevPractices and MaePractices corpora are no longer checked for
# here, because there is nothing on disk to check: they are compiled into the
# `mae` binary and built into stores on first run. Verifying their absence as
# `fail "manual KB missing"` — which is what this block did before the corpora
# were embedded — would now fail every single install.
ok "manual + guidance KBs: compiled into the binary"

# The ADR KB is the one that can legitimately be absent: it is not embedded
# (ADR-059 keeps it opt-in and contributor-only) and ships as its own release
# asset, so most packages do not carry it. Informational, never fatal.
if [ -e "$DATADIR/mae/mae-adr.cozo" ]; then
    ok "ADR KB present"
else
    skip "ADR KB not installed (opt-in — 'make fetch-adr-kb', then kb_register it)"
fi

MODULE_COUNT=$(find "$DATADIR/mae/modules" -name "module.toml" 2>/dev/null | wc -l | tr -d ' ')
if [ "$MODULE_COUNT" -ge 1 ]; then
    ok "$MODULE_COUNT modules installed"
else
    fail "no modules found"
fi

# Verify config
if [ -f "$CONFIGDIR/mae/config.toml" ]; then
    ok "config.toml present"
else
    warn "config.toml missing — run 'mae --init-config' to create one"
fi

# Verify .app bundle (macOS)
if [ "$OS" = "Darwin" ] && [ -d "$APP_DIR/MAE.app" ]; then
    if [ -x "$APP_DIR/MAE.app/Contents/MacOS/mae" ]; then
        ok "MAE.app installed to $APP_DIR"
    else
        fail "MAE.app present but binary not executable"
    fi
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
    printf "${RED}${BOLD}Completed with $ERRORS error(s)${RESET}\n"
fi

if [ "$WARNINGS" -gt 0 ]; then
    printf "${DIM}($WARNINGS warning(s) — see above)${RESET}\n"
fi

echo ""
if [ "$UPGRADE" -eq 0 ]; then
    printf "${BOLD}Getting started:${RESET}\n"
    if [ "$OS" = "Darwin" ] && [ -d "$APP_DIR/MAE.app" ]; then
        echo "  open $APP_DIR/MAE.app            # launch GUI"
    fi
    echo "  mae file.rs                      # open a file (GUI)"
    echo "  mae -nw file.rs                  # open a file (terminal)"
    echo ""

    if [ "$OS" = "Linux" ] && command -v systemctl >/dev/null 2>&1; then
        printf "${BOLD}Optional services:${RESET}\n"
        echo "  systemctl --user enable --now mae-daemon   # KB persistence + collab"
        echo ""
    elif [ "$OS" = "Darwin" ]; then
        printf "${BOLD}Optional services:${RESET}\n"
        echo "  launchctl load ~/Library/LaunchAgents/$LAUNCHD_LABEL.plist   # KB persistence + collab"
        echo ""
    fi

    printf "${BOLD}Learn more:${RESET}\n"
    echo "  :help tutorial:getting-started     # interactive tutorial"
    echo "  :help tutorial:ai-setup            # AI provider configuration"
    echo "  :help tutorial:collab-setup        # collaborative editing"
else
    printf "${BOLD}Manage:${RESET}\n"
    echo "  ./install.sh --uninstall           # remove MAE"
fi
echo ""
