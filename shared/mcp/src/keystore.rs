//! Trusted-key store for collab/daemon pre-shared-key (PSK) authentication.
//!
//! Symmetric PSKs are credentials and must **not** live in `config.toml`. They
//! belong in a permission-guarded `trusted_keys` file, modeled loosely on
//! OpenSSH's `authorized_keys`:
//!
//! - Default location: `$XDG_DATA_HOME/mae/collab/trusted_keys`
//!   (fallback `~/.local/share/mae/collab/trusted_keys`).
//! - Permissions: the file should be `0600` and its directory `0700`. A
//!   group/world-accessible keystore produces a loud warning (we still load it,
//!   unlike SSH which refuses, to stay forgiving for first-run setups).
//! - Format: one key per line; blank lines and `# comments` are ignored. Each
//!   key line is either `<secret>` (unnamed) or `<name> <secret>` (the name is
//!   used as the wire `key_id`, letting a daemon trust a SET of named peers).
//!
//! Both the editor (presents one key) and the daemon (trusts a set of keys)
//! read this file via `mae-mcp`, so the format and path stay in one place.

use std::path::{Path, PathBuf};

/// A single trusted key: an optional name plus the secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    /// Optional name. Sent as the `key_id` in the auth hello so a daemon can
    /// select the matching key from its trusted set. `None` = unnamed key.
    pub name: Option<String>,
    /// The pre-shared secret.
    pub secret: String,
}

/// A parsed keystore: the trusted-key entries plus the path they came from.
#[derive(Debug, Clone, Default)]
pub struct Keystore {
    /// Trusted-key entries, in file order.
    pub entries: Vec<KeyEntry>,
    /// The path the keystore was loaded from (empty for in-memory parses).
    pub path: PathBuf,
}

/// The default keystore path: `$XDG_DATA_HOME/mae/collab/trusted_keys`,
/// falling back to `~/.local/share/mae/collab/trusted_keys`.
///
/// Returns `None` only when neither `XDG_DATA_HOME` nor `HOME` is set.
pub fn default_keystore_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("mae/collab/trusted_keys"))
}

/// Parse keystore content. Pure (no I/O) so it is trivially testable.
///
/// Skips blank lines and `#` comments. Each remaining line is split on
/// whitespace: a single token is an unnamed secret; two-or-more tokens are
/// `name secret` (extra tokens are ignored — secrets never contain whitespace).
pub fn parse(content: &str) -> Vec<KeyEntry> {
    let mut entries = Vec::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut toks = line.split_whitespace();
        let first = match toks.next() {
            Some(t) => t,
            None => continue,
        };
        match toks.next() {
            // `name secret`
            Some(secret) => entries.push(KeyEntry {
                name: Some(first.to_string()),
                secret: secret.to_string(),
            }),
            // `secret` (unnamed)
            None => entries.push(KeyEntry {
                name: None,
                secret: first.to_string(),
            }),
        }
    }
    entries
}

/// Load and parse the keystore at `path`. The caller should first check
/// [`permission_warning`] and surface it. Missing files are an error
/// (`NotFound`); callers that treat "no keystore" as benign should check
/// `path.exists()` first or use [`load_optional`].
pub fn load(path: &Path) -> std::io::Result<Keystore> {
    let content = std::fs::read_to_string(path)?;
    Ok(Keystore {
        entries: parse(&content),
        path: path.to_path_buf(),
    })
}

/// Load the keystore at `path` if it exists; `Ok(None)` if it does not.
pub fn load_optional(path: &Path) -> std::io::Result<Option<Keystore>> {
    match load(path) {
        Ok(ks) => Ok(Some(ks)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

impl Keystore {
    /// The primary key — the first entry — which a client presents by default.
    pub fn primary(&self) -> Option<&KeyEntry> {
        self.entries.first()
    }

    /// Find a key by name.
    pub fn find(&self, name: &str) -> Option<&KeyEntry> {
        self.entries
            .iter()
            .find(|e| e.name.as_deref() == Some(name))
    }

    /// All secrets in the keystore (the daemon's trusted set).
    pub fn secrets(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.secret.as_str()).collect()
    }

    /// True when the keystore holds no keys.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of trusted keys.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// A warning string if the keystore file's permissions are too permissive
    /// (group- or world-accessible). `None` on non-unix or when perms are tight.
    pub fn permission_warning(&self) -> Option<String> {
        permission_warning(&self.path)
    }
}

/// Return a warning string if `path`'s permissions are group/world-accessible.
/// Unix only; always `None` elsewhere.
pub fn permission_warning(path: &Path) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path).ok()?;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            return Some(format!(
                "keystore {} has insecure permissions {:o} (should be 0600); \
                 run: chmod 600 {}",
                path.display(),
                mode & 0o7777,
                path.display()
            ));
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Generate a fresh random 256-bit secret, hex-encoded (64 chars).
pub fn generate_secret() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Append a key to the keystore at `path`, creating the directory (`0700`) and
/// file (`0600`) with secure permissions if needed. Refuses to add a duplicate
/// name. Returns the number of keys in the file afterwards.
pub fn add_key(path: &Path, name: Option<&str>, secret: &str) -> std::io::Result<usize> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        secure_dir(parent);
    }

    // Reject duplicate names so callers don't silently shadow a peer.
    let mut existing = load_optional(path)?.unwrap_or_default();
    if let Some(n) = name {
        if existing.find(n).is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("a key named '{n}' already exists in {}", path.display()),
            ));
        }
    }

    // Rewrite the whole file so we control the mode on first creation.
    existing.entries.push(KeyEntry {
        name: name.map(String::from),
        secret: secret.to_string(),
    });
    let body = render(&existing.entries);
    write_secure(path, &body)?;
    Ok(existing.entries.len())
}

/// Render keystore entries back to file content.
fn render(entries: &[KeyEntry]) -> String {
    let mut out = String::from("# MAE collab trusted keys — one per line: `[name] <secret>`\n");
    for e in entries {
        match &e.name {
            Some(n) => out.push_str(&format!("{n} {}\n", e.secret)),
            None => out.push_str(&format!("{}\n", e.secret)),
        }
    }
    out
}

/// Write `content` to `path`, restricted to the owner. This is the single
/// secret-file write path — the identity key, the PSK keystore, and per-KB content keys
/// all funnel through it — so the hardening here covers every key file at once.
///
/// @ai-caution: [security] The file is CREATED 0600, not created-then-chmodded.
/// The previous `fs::write(..)` + `chmod` pair (audit #608.4) created every
/// secret at `0666 & ~umask` — 0644 under the usual umask — and only narrowed
/// it afterwards, leaving a window in which any local user could open MAE's
/// private identity key, the PSK keystore, or a per-KB content key. The window
/// scales with the content size, so the keystore (the largest of the three) had
/// the widest one. The post-write `set_secure_file_perms` is still needed and
/// still runs: `mode()` applies only at creation, so it is what tightens a
/// *pre-existing* file that was written before this fix.
pub fn write_secure(path: &Path, content: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(content.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, content)?;
    set_secure_file_perms(path)
}

/// [`write_secure`], but all-or-nothing: the content goes to a fresh temporary beside
/// `path` ([`write_secure_new`]), is flushed to disk, and is renamed over `path`. A crash at
/// any point leaves either the old file or the new one, never a truncated one. For a key file
/// that is the only copy of a key — a half-written one is a lost key.
///
/// @ai-caution: [security] The temporary's name is unique per write and it is opened
/// `create_new` (`O_EXCL`) + `O_NOFOLLOW`. It used to be a fixed `<name>.tmp` opened
/// create+truncate: a symlink planted there received the secret, and two concurrent writers
/// interleaved in one file. Never go back to a predictable, reusable temporary.
pub fn write_secure_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = unique_tmp_path(path)?;
    write_secure_new(&tmp, content)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    sync_parent(path);
    Ok(())
}

/// Create `path` holding `content`, 0600 from creation, and fail with `AlreadyExists` if
/// anything is already there — a file, or a symlink, dangling or not. All-or-nothing like
/// [`write_secure_atomic`], but never replaces: of two processes creating the same file, at
/// most one succeeds, and an existing file is left byte-for-byte as it was. (The content is
/// written to a fresh temporary, then hard-linked into place: `link(2)` does not replace.)
pub fn create_secure_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    let tmp = unique_tmp_path(path)?;
    write_secure_new(&tmp, content)?;
    let linked = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    linked?;
    sync_parent(path);
    Ok(())
}

/// A temporary name beside `path` that no other write uses: the process id, a per-process
/// counter and a random suffix (the random part keeps a stale temporary left by a crashed
/// process from blocking a later process that reuses its pid).
fn unique_tmp_path(path: &Path) -> std::io::Result<PathBuf> {
    use rand::Rng;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name")
    })?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(
        ".{}.{}.{:016x}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
        rand::rng().next_u64()
    ));
    Ok(path.with_file_name(tmp_name))
}

/// Create a NEW file at `path` holding `content`, owner-only from creation and flushed to
/// disk. Refuses anything already at `path`: `create_new` is `O_CREAT|O_EXCL`, which fails
/// on an existing name — a symlink included, and does not follow it — and on unix
/// `O_NOFOLLOW` refuses a symlink as the last component as well (belt and braces). On any
/// failure after the file was created it is removed: no partial secret is left behind.
fn write_secure_new(path: &Path, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    // Nothing was created if the open fails, so there is nothing to remove.
    let mut f = opts.open(path)?;
    let written = f
        .write_all(content.as_bytes())
        .and_then(|()| f.sync_all())
        .and_then(|()| secure_open_file(&f, path));
    drop(f);
    if let Err(e) = written {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}

/// [`set_secure_file_perms`] on the open handle (`fchmod`), not the name: the name could
/// have been swapped since the open, the handle cannot. The mode is already 0600 from
/// creation; this pins it against a umask that would only have narrowed it further.
fn secure_open_file(f: &std::fs::File, path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = path;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = f;
        warn_unenforced_perms(path);
        Ok(())
    }
}

/// A rename or link is durable only once its directory is flushed (unix).
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Restrict a freshly-written secret file to the owner. On unix: `chmod 0600` (a failure
/// propagates — we never leave a secret world-readable silently). On non-unix (#158 I4,
/// principle #13): MAE cannot portably set an owner-only ACL without a platform crate, so
/// rather than **silently no-op** on one platform we **warn once** — the operator must
/// rely on directory/user ACLs until native ACL tightening lands (a documented follow-up;
/// MAE ships no Windows build yet, so this gates a future Windows release, not today's).
fn set_secure_file_perms(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    warn_unenforced_perms(path);
    Ok(())
}

/// Warn ONCE that a secret file could not be permission-restricted. Kept platform-
/// agnostic and **always compiled** (so Linux CI type-checks it — there is no Windows CI
/// matrix), but only *called* on non-unix by [`set_secure_file_perms`].
#[cfg_attr(unix, allow(dead_code))]
fn warn_unenforced_perms(path: &Path) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            path = %path.display(),
            "secret key files are written WITHOUT an enforced owner-only ACL on this \
             platform (#158 I4): MAE cannot set 0600-equivalent permissions here. Protect \
             the mae data directory via filesystem ACLs; native ACL tightening is a \
             follow-up. (On unix, key files are chmod 0600 automatically.)"
        );
    }
}

#[cfg(unix)]
fn secure_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn secure_dir(_dir: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// #158 I4: the non-unix permission-advisory body is always compiled (there is no
    /// Windows CI matrix), so call it on Linux to catch a typo there. Warn-once ⇒ a
    /// second call is a silent no-op (no panic).
    #[test]
    fn unenforced_perms_warning_is_callable_on_all_platforms() {
        warn_unenforced_perms(std::path::Path::new("/nonexistent/key"));
        warn_unenforced_perms(std::path::Path::new("/nonexistent/key"));
    }

    /// Audit #608.4 — the attacker's test. Secrets were written with
    /// `fs::write` (creating the file `0666 & ~umask`, i.e. world-readable
    /// under the usual 022) and only chmodded to 0600 *afterwards*. Any local
    /// user polling the path during that window could open MAE's private
    /// identity key, the PSK keystore, or a per-KB content key.
    ///
    /// A second thread polls the mode while a large secret is written. The
    /// oracle is one-directional by construction: correct code can NEVER be
    /// observed permissive (the file is created 0600), so this test cannot
    /// produce a false failure — while the old code loses the race with high
    /// probability at this payload size.
    #[cfg(unix)]
    #[test]
    fn a_secret_is_never_observable_with_permissive_mode_while_being_written() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join("mae-keystore-toctou");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.key");
        let _ = std::fs::remove_file(&path);

        // Large enough that the write is not a single instantaneous syscall.
        let secret = "s3cr3t-".repeat(2_000_000);

        let done = Arc::new(AtomicBool::new(false));
        let leaked = Arc::new(AtomicBool::new(false));
        let poller = {
            let (path, done, leaked) = (path.clone(), done.clone(), leaked.clone());
            std::thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if let Ok(md) = std::fs::metadata(&path) {
                        // Any group or other bit set at any instant is a leak.
                        if md.permissions().mode() & 0o077 != 0 {
                            leaked.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                    std::hint::spin_loop();
                }
            })
        };

        write_secure(&path, &secret).expect("write_secure");
        done.store(true, Ordering::Relaxed);
        poller.join().unwrap();

        assert!(
            !leaked.load(Ordering::Relaxed),
            "the secret at {} was observable with group/other permissions while \
             being written",
            path.display()
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "final mode must still be owner-only"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The complementary case the `mode()` flag alone does NOT cover: an
    /// already-existing, already-permissive file must be tightened on rewrite,
    /// because `OpenOptions::mode` applies at CREATION only. This is why the
    /// post-write `set_secure_file_perms` call has to stay.
    #[cfg(unix)]
    #[test]
    fn rewriting_a_preexisting_world_readable_secret_tightens_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("mae-keystore-preexisting");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.key");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_secure(&path, "new-secret").expect("write_secure");

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "a legacy world-readable key file must be tightened on rewrite"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-secret");
        let _ = std::fs::remove_file(&path);
    }

    /// The atomic write replaces a pre-existing, permissive file with an owner-only one
    /// holding the new content, and leaves no temporary behind.
    #[cfg(unix)]
    #[test]
    fn an_atomic_write_replaces_whole_and_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("id_ed25519");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_secure_atomic(&path, "new-secret").expect("write_secure_atomic");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new-secret");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            dir_names(tmp.path()),
            vec!["id_ed25519"],
            "no temporary left behind"
        );
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// The attacker's test: a local user plants a symlink where the secret's temporary
    /// would go, pointing at a file they can read. The secret must never be written through
    /// it. Planted at the old fixed temporary name, a write must succeed without touching
    /// it; handed a symlink as the temporary itself, the create must refuse.
    #[cfg(unix)]
    #[test]
    fn a_planted_symlink_never_receives_the_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let bait = tmp.path().join("attacker-readable");
        std::fs::write(&bait, "bait").unwrap();
        let path = tmp.path().join("id_ed25519");
        let legacy = tmp.path().join("id_ed25519.tmp");
        std::os::unix::fs::symlink(&bait, &legacy).unwrap();

        write_secure_atomic(&path, "s3cret").expect("the write succeeds");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret");
        assert_eq!(
            std::fs::read_to_string(&bait).unwrap(),
            "bait",
            "written through"
        );
        assert_eq!(
            std::fs::read_link(&legacy).unwrap(),
            bait,
            "the plant is untouched"
        );

        // The temporary itself is a symlink — to a file, and dangling.
        let link = tmp.path().join("planted");
        std::os::unix::fs::symlink(&bait, &link).unwrap();
        assert!(write_secure_new(&link, "s3cret").is_err());
        assert_eq!(
            std::fs::read_to_string(&bait).unwrap(),
            "bait",
            "written through"
        );
        let gone = tmp.path().join("not-yet");
        let dangling = tmp.path().join("dangling");
        std::os::unix::fs::symlink(&gone, &dangling).unwrap();
        assert!(write_secure_new(&dangling, "s3cret").is_err());
        assert!(!gone.exists(), "created through a dangling symlink");
    }

    /// Two writers of one secret file never share a temporary: each write uses its own.
    #[test]
    fn concurrent_atomic_writes_each_land_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rotation_pending");
        let bodies: Vec<String> = (0..8).map(|i| format!("{i}-").repeat(50_000)).collect();
        std::thread::scope(|s| {
            for b in &bodies {
                let path = &path;
                s.spawn(move || write_secure_atomic(path, b).expect("write"));
            }
        });
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(bodies.contains(&got), "an interleaved file");
        assert_eq!(dir_names(tmp.path()), vec!["rotation_pending"]);
    }

    /// A no-replace create: the first wins, a second finds the file there and fails,
    /// and the first is left byte for byte — also when the name is a dangling symlink.
    #[test]
    fn a_create_never_replaces_what_is_there() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rotation_pending");
        create_secure_atomic(&path, "first").expect("the first create");
        let e = create_secure_atomic(&path, "second").expect_err("a second create");
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        assert_eq!(
            dir_names(tmp.path()),
            vec!["rotation_pending"],
            "no temporary left"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = tmp.path().join("other");
            let gone = tmp.path().join("gone");
            std::os::unix::fs::symlink(&gone, &link).unwrap();
            assert!(create_secure_atomic(&link, "x").is_err());
            assert!(!gone.exists());
        }
    }

    #[test]
    fn parse_skips_comments_and_blanks() {
        let ks = parse("# comment\n\n   \nsecret-only\n");
        assert_eq!(ks.len(), 1);
        assert_eq!(ks[0].name, None);
        assert_eq!(ks[0].secret, "secret-only");
    }

    #[test]
    fn parse_named_and_unnamed() {
        let ks = parse("laptop abc123\ndeadbeef\n# x\nphone  def456  \n");
        assert_eq!(ks.len(), 3);
        assert_eq!(
            ks[0],
            KeyEntry {
                name: Some("laptop".into()),
                secret: "abc123".into()
            }
        );
        assert_eq!(
            ks[1],
            KeyEntry {
                name: None,
                secret: "deadbeef".into()
            }
        );
        assert_eq!(
            ks[2],
            KeyEntry {
                name: Some("phone".into()),
                secret: "def456".into()
            }
        );
    }

    #[test]
    fn keystore_lookup_helpers() {
        let ks = Keystore {
            entries: parse("a key-a\nb key-b\n"),
            path: PathBuf::new(),
        };
        assert_eq!(ks.primary().unwrap().secret, "key-a");
        assert_eq!(ks.find("b").unwrap().secret, "key-b");
        assert!(ks.find("c").is_none());
        assert_eq!(ks.secrets(), vec!["key-a", "key-b"]);
    }

    #[test]
    fn generate_secret_is_64_hex_chars() {
        let s = generate_secret();
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(
            generate_secret(),
            generate_secret(),
            "secrets must be random"
        );
    }

    #[test]
    fn add_key_creates_secure_file_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("mae-ks-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("collab/trusted_keys");

        let n = add_key(&path, Some("laptop"), "secret-1").unwrap();
        assert_eq!(n, 1);
        let n = add_key(&path, None, "secret-2").unwrap();
        assert_eq!(n, 2);

        let ks = load(&path).unwrap();
        assert_eq!(ks.len(), 2);
        assert_eq!(ks.find("laptop").unwrap().secret, "secret-1");
        assert_eq!(
            ks.entries[1],
            KeyEntry {
                name: None,
                secret: "secret-2".into()
            }
        );

        // Duplicate name is rejected.
        let err = add_key(&path, Some("laptop"), "secret-3").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

        // File is 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "keystore file must be 0600");
            assert!(permission_warning(&path).is_none());
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_optional_missing_is_none() {
        let path = std::env::temp_dir().join("mae-ks-does-not-exist-xyz/trusted_keys");
        assert!(load_optional(&path).unwrap().is_none());
    }
}
