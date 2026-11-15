//! The identity-provider binding table (ADR-098 D3, Amendment 1 C1–C3).
//!
//! Maps an external identity provider's `(iss, sub)` to a member fingerprint,
//! one to one in both directions, with the `kind` (`person` | `agent`) that
//! decides who counts as a person (C3) and how the binding was made (`proof`
//! by a signed challenge, or `admin` by the local CLI).
//!
//! Daemon state, outside the CRDT: an IdP migration rewrites this table, never
//! a signed log (D3). It lives in its own SQLite file next to `state.db`
//! ([`BINDINGS_FILE`]) rather than inside it, because the local administrator
//! changes it while the daemon serves: `state.db` is held under the store lock
//! for the daemon's life, while this table is never cached in memory, so a
//! change made by `mae-daemon binding …` in another process takes effect on
//! the listener's next request. SQLite's own locking (WAL, `busy_timeout`,
//! `BEGIN IMMEDIATE`) serialises the two writers.
//!
//! @ai-caution: [security] Only two writers may exist: the local CLI
//! (`binding_cli`, principle #16) and the proof-of-possession route
//! (`oauth_binding`), which can only ADD a binding for the presenting token's
//! own subject after a verified signature. No network method may unbind, rebind
//! or change a `kind`.

use std::path::Path;
use std::sync::Mutex;

use crate::oauth_self_issue::{Credential, PrincipalKind, ValidatedPrincipal};

/// The bindings file, in the collab data directory beside `state.db`.
pub const BINDINGS_FILE: &str = "identity-bindings.db";

/// How a binding was made: by a verified signature over a challenge, or by the
/// local administrator without one (C2: shown as such in `binding list`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundBy {
    Proof,
    Admin,
}

impl BoundBy {
    pub fn as_str(self) -> &'static str {
        match self {
            BoundBy::Proof => "proof",
            BoundBy::Admin => "admin",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "proof" => Some(BoundBy::Proof),
            "admin" => Some(BoundBy::Admin),
            _ => None,
        }
    }
}

/// One row of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub issuer: String,
    pub subject: String,
    pub fingerprint: String,
    pub kind: PrincipalKind,
    pub bound_by: BoundBy,
    /// Unix seconds.
    pub bound_at: i64,
}

/// Why a write was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// This `(iss, sub)` is already bound; unbind it first (C2).
    SubjectBound,
    /// This fingerprint is already bound to a subject; unbind it first (C2,
    /// D3's "never bind a fingerprint another principal holds").
    FingerprintBound,
    /// No binding matches.
    NotFound,
    /// The fingerprint is not the shape of a member key's ([`is_fingerprint`]),
    /// or the subject is empty.
    Malformed,
    Storage(String),
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SubjectBound => write!(f, "this subject is already bound; unbind it first"),
            Self::FingerprintBound => {
                write!(f, "this fingerprint is already bound; unbind it first")
            }
            Self::NotFound => write!(f, "no such binding"),
            Self::Malformed => write!(
                f,
                "the fingerprint must be a member key's (SHA256:…) and the subject non-empty"
            ),
            Self::Storage(e) => write!(f, "binding store: {e}"),
        }
    }
}

impl From<sqlite::Error> for BindingError {
    fn from(e: sqlite::Error) -> Self {
        BindingError::Storage(e.to_string())
    }
}

/// The PRIMARY KEY and UNIQUE constraints are the one-to-one rule itself, so a
/// writer that skipped [`BindingStore::bind`]'s checks still cannot break it.
const SCHEMA: &str = "PRAGMA journal_mode=WAL;
     PRAGMA busy_timeout=5000;
     CREATE TABLE IF NOT EXISTS identity_bindings (
         issuer TEXT NOT NULL,
         subject TEXT NOT NULL,
         fingerprint TEXT NOT NULL UNIQUE,
         kind TEXT NOT NULL DEFAULT 'agent' CHECK (kind IN ('person', 'agent')),
         bound_by TEXT NOT NULL CHECK (bound_by IN ('proof', 'admin')),
         bound_at INTEGER NOT NULL,
         PRIMARY KEY (issuer, subject)
     );";

/// `SHA256:` and 43 characters of unpadded standard base64: the only shape
/// `PublicKey::fingerprint` produces. Both writers refuse anything else, so a
/// binding can never name a principal that is not a key.
pub fn is_fingerprint(s: &str) -> bool {
    s.strip_prefix("SHA256:").is_some_and(|b64| {
        b64.len() == 43
            && b64
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/')
    })
}

/// Which binding a CLI action names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    Subject { issuer: String, subject: String },
    Fingerprint(String),
}

/// The table, behind one connection.
pub struct BindingStore {
    conn: Mutex<sqlite::Connection>,
}

impl BindingStore {
    /// Open (creating if absent) the bindings file at `path`.
    pub fn open(path: &Path) -> Result<Self, BindingError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| BindingError::Storage(e.to_string()))?;
        }
        Self::from_conn(sqlite::Connection::open(path)?)
    }

    /// An in-memory table, for tests.
    pub fn open_memory() -> Result<Self, BindingError> {
        Self::from_conn(sqlite::Connection::open(":memory:")?)
    }

    fn from_conn(conn: sqlite::Connection) -> Result<Self, BindingError> {
        conn.execute(SCHEMA)?;
        Ok(BindingStore {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, sqlite::Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The binding of `(issuer, subject)`, read from the file on every call.
    pub fn lookup(&self, issuer: &str, subject: &str) -> Result<Option<Binding>, BindingError> {
        let sel = Selector::Subject {
            issuer: issuer.to_string(),
            subject: subject.to_string(),
        };
        Ok(select(&self.lock(), Some(&sel))?.into_iter().next())
    }

    /// Every binding, ordered by issuer then subject.
    pub fn list(&self) -> Result<Vec<Binding>, BindingError> {
        select(&self.lock(), None)
    }

    /// Bind `(issuer, subject)` to `fingerprint` as an `agent` (C3: a binding
    /// is an agent until an administrator sets it). Refused if either side is
    /// already bound: replacing a binding takes an explicit unbind first.
    pub fn bind(
        &self,
        issuer: &str,
        subject: &str,
        fingerprint: &str,
        bound_by: BoundBy,
        now: i64,
    ) -> Result<(), BindingError> {
        if !is_fingerprint(fingerprint) || subject.is_empty() {
            return Err(BindingError::Malformed);
        }
        let conn = self.lock();
        conn.execute("BEGIN IMMEDIATE")?;
        let result = bind_in_tx(&conn, issuer, subject, fingerprint, bound_by, now);
        finish_tx(&conn, result)
    }

    /// Remove the binding `sel` names. `NotFound` when none does.
    pub fn unbind(&self, sel: &Selector) -> Result<Binding, BindingError> {
        let conn = self.lock();
        conn.execute("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Binding, BindingError> {
            let found = select(&conn, Some(sel))?
                .into_iter()
                .next()
                .ok_or(BindingError::NotFound)?;
            let mut st =
                conn.prepare("DELETE FROM identity_bindings WHERE issuer = ?1 AND subject = ?2")?;
            st.bind((1, found.issuer.as_str()))?;
            st.bind((2, found.subject.as_str()))?;
            while st.next()? != sqlite::State::Done {}
            Ok(found)
        })();
        finish_tx(&conn, result)
    }

    /// Set the `kind` of the binding `sel` names.
    pub fn set_kind(&self, sel: &Selector, kind: PrincipalKind) -> Result<Binding, BindingError> {
        let conn = self.lock();
        conn.execute("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<Binding, BindingError> {
            let mut found = select(&conn, Some(sel))?
                .into_iter()
                .next()
                .ok_or(BindingError::NotFound)?;
            let mut st = conn.prepare(
                "UPDATE identity_bindings SET kind = ?1 WHERE issuer = ?2 AND subject = ?3",
            )?;
            st.bind((1, kind.as_str()))?;
            st.bind((2, found.issuer.as_str()))?;
            st.bind((3, found.subject.as_str()))?;
            while st.next()? != sqlite::State::Done {}
            found.kind = kind;
            Ok(found)
        })();
        finish_tx(&conn, result)
    }
}

fn finish_tx<T>(
    conn: &sqlite::Connection,
    result: Result<T, BindingError>,
) -> Result<T, BindingError> {
    match result {
        Ok(v) => {
            if let Err(e) = conn.execute("COMMIT") {
                // A failed COMMIT can leave the transaction open (SQLite keeps
                // it on SQLITE_BUSY and on a deferred constraint). Left so,
                // every later `BEGIN IMMEDIATE` on this connection fails.
                let _ = conn.execute("ROLLBACK");
                return Err(e.into());
            }
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK");
            Err(e)
        }
    }
}

fn bind_in_tx(
    conn: &sqlite::Connection,
    issuer: &str,
    subject: &str,
    fingerprint: &str,
    bound_by: BoundBy,
    now: i64,
) -> Result<(), BindingError> {
    let by_subject = Selector::Subject {
        issuer: issuer.to_string(),
        subject: subject.to_string(),
    };
    if !select(conn, Some(&by_subject))?.is_empty() {
        return Err(BindingError::SubjectBound);
    }
    let by_fp = Selector::Fingerprint(fingerprint.to_string());
    if !select(conn, Some(&by_fp))?.is_empty() {
        return Err(BindingError::FingerprintBound);
    }
    let mut st = conn.prepare(
        "INSERT INTO identity_bindings (issuer, subject, fingerprint, kind, bound_by, bound_at) \
         VALUES (?1, ?2, ?3, 'agent', ?4, ?5)",
    )?;
    st.bind((1, issuer))?;
    st.bind((2, subject))?;
    st.bind((3, fingerprint))?;
    st.bind((4, bound_by.as_str()))?;
    st.bind((5, now))?;
    while st.next()? != sqlite::State::Done {}
    Ok(())
}

fn select(conn: &sqlite::Connection, sel: Option<&Selector>) -> Result<Vec<Binding>, BindingError> {
    const COLS: &str = "SELECT issuer, subject, fingerprint, kind, bound_by, bound_at \
                        FROM identity_bindings";
    let mut st = match sel {
        None => conn.prepare(format!("{COLS} ORDER BY issuer, subject"))?,
        Some(Selector::Subject { issuer, subject }) => {
            let mut st = conn.prepare(format!("{COLS} WHERE issuer = ?1 AND subject = ?2"))?;
            st.bind((1, issuer.as_str()))?;
            st.bind((2, subject.as_str()))?;
            st
        }
        Some(Selector::Fingerprint(fp)) => {
            let mut st = conn.prepare(format!("{COLS} WHERE fingerprint = ?1"))?;
            st.bind((1, fp.as_str()))?;
            st
        }
    };
    let mut out = Vec::new();
    while st.next()? == sqlite::State::Row {
        let bad = |col: &str, v: &str| BindingError::Storage(format!("bad {col} '{v}'"));
        let kind: String = st.read("kind")?;
        let bound_by: String = st.read("bound_by")?;
        out.push(Binding {
            issuer: st.read("issuer")?,
            subject: st.read("subject")?,
            fingerprint: st.read("fingerprint")?,
            kind: PrincipalKind::parse(&kind).ok_or_else(|| bad("kind", &kind))?,
            bound_by: BoundBy::parse(&bound_by).ok_or_else(|| bad("bound_by", &bound_by))?,
            bound_at: st.read("bound_at")?,
        });
    }
    Ok(out)
}

/// Resolve an external token's principal through the table (C1, C3).
///
/// Only an `IdpUnbound` principal is looked up: a self-issued token's
/// principal already is a fingerprint, and it is an agent whatever the
/// fingerprint's binding says (C3 needs an IdP sign-in). A bound subject
/// becomes the bound fingerprint, a person only if the binding is flagged so.
/// With no store, or no binding, the principal stays an unbound subject —
/// a member of nothing.
pub fn resolve(
    vp: ValidatedPrincipal,
    store: Option<&BindingStore>,
) -> Result<ValidatedPrincipal, BindingError> {
    if vp.credential != Credential::IdpUnbound {
        return Ok(vp);
    }
    let Some(store) = store else {
        return Ok(vp);
    };
    Ok(match store.lookup(&vp.issuer, &vp.subject)? {
        Some(b) => ValidatedPrincipal {
            principal: b.fingerprint,
            credential: Credential::IdpBound,
            kind: b.kind,
            ..vp
        },
        None => vp,
    })
}

/// [`resolve`] for the async listener: the lookup is blocking SQLite behind a
/// mutex, and a CLI writer may hold the file for up to `busy_timeout`, so it
/// runs on the blocking pool rather than stalling a runtime worker. A lookup
/// task that dies is a storage error — the caller fails closed on it.
pub async fn resolve_off_runtime(
    vp: ValidatedPrincipal,
    store: Option<std::sync::Arc<BindingStore>>,
) -> Result<ValidatedPrincipal, BindingError> {
    match store {
        // Only an unbound IdP principal is looked up; the rest need no hop.
        Some(store) if vp.credential == Credential::IdpUnbound => {
            tokio::task::spawn_blocking(move || resolve(vp, Some(&store)))
                .await
                .map_err(|e| BindingError::Storage(format!("binding lookup task: {e}")))?
        }
        store => resolve(vp, store.as_deref()),
    }
}

#[cfg(test)]
#[path = "identity_binding_tests.rs"]
mod identity_binding_tests;
