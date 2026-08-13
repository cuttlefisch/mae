#!/bin/sh
# backup-kbs.sh — archive the user's KBs, verify the archive, and only then
# remove the originals.
#
# `make uninstall` removes the binaries, desktop entry, icon and modules and
# would otherwise leave the mae data dir behind. Deleting it outright is the
# wrong correction: a KB is user data, and most of it (a customised practices
# KB, a federated instance, the registry recording where they came from) exists
# nowhere else.
#
# So: archive first, VERIFY the archive, and remove the originals only if the
# verification passed.
#
# So: archive first, VERIFY the archive, and remove the originals only if the
# verification passed. Any failure leaves everything exactly where it was.
# Modelled on reset-collab-state.sh, which established the house conventions
# here (POSIX sh, XDG-first dir resolution on every platform, "move aside,
# never delete").
#
# Usage:
#   scripts/backup-kbs.sh                 # prompt for a destination, default $PWD
#   MAE_BACKUP_DIR=/path scripts/backup-kbs.sh   # no prompt
#   scripts/backup-kbs.sh --list          # show what would be archived
#
# Exit: 0 on success or nothing-to-do; non-zero if the archive could not be
# written or verified (originals kept).

set -eu

# What is worth archiving, discovered per base rather than hard-coded.
#
# This used to be a fixed list of the four stores `make install` wrote:
#   mae-manual.cozo mae-practices.cozo mae-devpractices.cozo mae-adr.cozo
# which had it exactly backwards. Those are MAE's own corpora — every one of
# them is regenerable, and three are now built automatically from sources
# compiled into the binary, so they are the *only* KB content in the data dir
# that is never worth protecting. Meanwhile a user's own registered KBs, and
# the registry that records them, were ignored entirely: the script would
# happily report "nothing to archive" while standing in a directory full of
# irreplaceable notes.
#
# So: archive everything KB-shaped EXCEPT the regenerable system stores.
# Unknown names are included by default, which is the safe direction for a
# script whose next step is `rm -rf` — a stale MAE store that slips through
# costs disk, whereas a missed user KB costs data.
#
# `mae-adr.cozo` is deliberately still archived even though `make adr-kb` can
# rebuild it: that requires a checkout of MAE's own sources, which an end user
# installing from a release does not have.
REGENERABLE="mae-manual.cozo mae-practices.cozo mae-devpractices.cozo"

# Echo the archivable entry names in $1, one per line.
#
# Word-splitting on the result is the established contract in this script, so
# the names must not contain whitespace. MAE only ever creates `*.cozo` stores
# and `kb-registry.toml` here; anything stranger is skipped rather than risk
# producing a member list that tar would misread.
kb_items_in() {
    _base="$1"
    [ -f "$_base/kb-registry.toml" ] && printf '%s\n' "kb-registry.toml"
    for _p in "$_base"/*.cozo; do
        [ -e "$_p" ] || continue
        _n="${_p##*/}"
        case " $REGENERABLE " in *" $_n "*) continue ;; esac
        case "$_n" in *[[:space:]]*)
            echo "backup-kbs: skipping '$_n' — whitespace in the name" >&2
            continue ;;
        esac
        printf '%s\n' "$_n"
    done
}

# XDG-first on EVERY platform, then the platform default (CLAUDE.md #13).
# Deliberately not `dirs::data_dir()` semantics: that ignores XDG on macOS and
# would contradict the documented ~/.local/share/mae contract.
DIRS="$HOME/.local/share/mae"
[ -n "${XDG_DATA_HOME:-}" ] && DIRS="$XDG_DATA_HOME/mae $DIRS"
[ "$(uname -s)" = "Darwin" ] && DIRS="$DIRS $HOME/Library/Application Support/mae"

# De-duplicate (XDG_DATA_HOME may equal the default) and keep only real dirs.
seen=" "
BASES=""
for base in $DIRS; do
    case "$seen" in *" $base "*) continue ;; esac
    seen="$seen$base "
    [ -d "$base" ] && BASES="$BASES$base
"
done

# Exactly ONE base dir is acted on: the first candidate that actually holds KBs
# (XDG first, so an XDG_DATA_HOME override wins over the platform default).
#
# Deliberately single-base. An earlier draft collected candidates from every
# base for `--list` but archived only the first, so `--list` promised eight
# paths across two dirs and the run touched four in one — it named the
# contributor's real ~/.local/share/mae while operating on an isolated XDG dir.
# For a command whose next step is `rm -rf`, a listing that overstates its own
# scope is worse than no listing.
target_base=""
for base in $BASES; do
    for kb in $(kb_items_in "$base"); do
        if [ -e "$base/$kb" ]; then
            target_base="$base"
            break
        fi
    done
    [ -n "$target_base" ] && break
done

if [ -z "$target_base" ]; then
    echo "backup-kbs: no installed KBs found — nothing to archive."
    exit 0
fi

# Say so when another candidate also holds KBs: only one is being handled, and
# silence would read as "there was only one".
for base in $BASES; do
    [ "$base" = "$target_base" ] && continue
    for kb in $(kb_items_in "$base"); do
        [ -e "$base/$kb" ] || continue
        echo "backup-kbs: note — $base also holds KBs; this run only handles $target_base"
        break
    done
done

if [ "${1:-}" = "--list" ]; then
    echo "backup-kbs: would archive from $target_base:"
    for kb in $(kb_items_in "$target_base"); do
        [ -e "$target_base/$kb" ] && echo "  $kb"
    done
    exit 0
fi

# Destination. A prompt must never hang an unattended run: `make uninstall` has
# to stay scriptable, so a non-TTY stdin (or an explicit env var) skips it.
default_dest="$PWD"
if [ -n "${MAE_BACKUP_DIR:-}" ]; then
    dest="$MAE_BACKUP_DIR"
elif [ -t 0 ]; then
    printf 'Where should the KB backup be written? [%s]: ' "$default_dest"
    read -r answer || answer=""
    dest="${answer:-$default_dest}"
else
    dest="$default_dest"
    echo "backup-kbs: stdin is not a terminal — using $dest (set MAE_BACKUP_DIR to override)"
fi

mkdir -p "$dest" 2>/dev/null || {
    echo "backup-kbs: cannot create destination '$dest' — originals kept." >&2
    exit 1
}
[ -w "$dest" ] || {
    echo "backup-kbs: destination '$dest' is not writable — originals kept." >&2
    exit 1
}

TS="$(date -u +%Y%m%d-%H%M%S)"
archive="$dest/mae-kbs-$TS.tar.gz"

# Build the member list relative to each base so the tarball unpacks cleanly.
tmplist="$(mktemp)"
trap 'rm -f "$tmplist"' EXIT INT TERM
: > "$tmplist"
for kb in $(kb_items_in "$target_base"); do
    [ -e "$target_base/$kb" ] || continue
    printf '%s\n' "$kb" >> "$tmplist"
    [ -e "$target_base/$kb.sha256" ] && printf '%s\n' "$kb.sha256" >> "$tmplist"
done
tar czf "$archive" -C "$target_base" -T "$tmplist" || {
    echo "backup-kbs: tar failed — originals kept." >&2
    rm -f "$archive"
    exit 1
}
archived_base="$target_base"

# Verify BEFORE deleting anything. Same sha256 fallback the fetch-adr-kb target
# uses (sha256sum on Linux, shasum on macOS).
if command -v sha256sum >/dev/null 2>&1; then
    ( cd "$dest" && sha256sum "$(basename "$archive")" > "$archive.sha256" )
    verify="sha256sum -c"
elif command -v shasum >/dev/null 2>&1; then
    ( cd "$dest" && shasum -a 256 "$(basename "$archive")" > "$archive.sha256" )
    verify="shasum -a 256 -c"
else
    echo "backup-kbs: no sha256sum/shasum — cannot verify the archive, so the" >&2
    echo "            originals are being kept. Archive written to $archive" >&2
    exit 1
fi

if ! ( cd "$dest" && $verify "$(basename "$archive").sha256" >/dev/null 2>&1 ); then
    echo "backup-kbs: checksum verification FAILED — originals kept." >&2
    exit 1
fi

# And prove every expected member is actually inside it.
missing=""
while IFS= read -r member; do
    tar tzf "$archive" | grep -qxF "$member" || \
        tar tzf "$archive" | grep -q "^$member/" || missing="$missing $member"
done < "$tmplist"
if [ -n "$missing" ]; then
    echo "backup-kbs: archive is missing:$missing — originals kept." >&2
    exit 1
fi

echo "backup-kbs: archived and verified -> $archive"

# Verified. Now, and only now, remove the originals.
while IFS= read -r member; do
    rm -rf "${archived_base:?}/$member"
done < "$tmplist"
echo "backup-kbs: removed the installed copies under $archived_base"
echo "backup-kbs: restore with:  tar xzf \"$archive\" -C \"$archived_base\""
