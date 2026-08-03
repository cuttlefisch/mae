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
