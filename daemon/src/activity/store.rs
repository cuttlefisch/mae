//! `activity.db` (ADR-120 D3): its schema, its append-only triggers, and the
//! one append transaction every writer uses — the daemon's writer task and an
//! operator command in another process alike.
//!
//! @ai-caution: [concurrency] Each chain's tip, the global sequence and the
//! latest committed `at` are read INSIDE the `BEGIN IMMEDIATE` transaction
//! that appends after them, never from a cache (D5). An operator command
//! appends from another process; a cached tip would fork a chain, or here,
//! with `UNIQUE (chain, seq)`, refuse the daemon's next batch.

use std::path::Path;

use super::chain::{self, seal, Link, Sealed, ZERO_HASH};
use super::event::{Chain, Columns, Event, EventKind};

/// How long a writer waits for another process's write lock before its
/// batch fails and is retried: longer than an operator command holds it.
pub const BUSY_TIMEOUT_MS: u64 = 5000;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(String),
    Io(String),
    Corrupt(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "activity.db: {e}"),
            StoreError::Io(e) => write!(f, "activity.db: {e}"),
            StoreError::Corrupt(e) => write!(f, "activity.db is not a valid record: {e}"),
        }
    }
}

impl From<sqlite::Error> for StoreError {
    fn from(e: sqlite::Error) -> Self {
        StoreError::Sqlite(e.to_string())
    }
}

/// The schema. The columns beside `link` and `body` are an index derived from
/// the body and never a source (D3): export filters on them, and verification
/// recomputes each and fails on a mismatch.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS events (
    gseq INTEGER PRIMARY KEY,
    chain TEXT NOT NULL CHECK (chain IN ('long', 'short')),
    seq INTEGER NOT NULL,
    link BLOB NOT NULL,
    body BLOB NOT NULL,
    at TEXT NOT NULL,
    principal_id TEXT,
    kb_id TEXT,
    kind TEXT NOT NULL,
    outcome TEXT NOT NULL,
    outcome_kind TEXT,
    session TEXT,
    tenant TEXT,
    UNIQUE (chain, seq)
);
CREATE INDEX IF NOT EXISTS events_at ON events (at);
CREATE INDEX IF NOT EXISTS events_principal ON events (principal_id);
CREATE INDEX IF NOT EXISTS events_kb ON events (kb_id);
CREATE INDEX IF NOT EXISTS events_session ON events (session);
CREATE TABLE IF NOT EXISTS heads (
    gseq INTEGER PRIMARY KEY,
    canonical BLOB NOT NULL,
    sig BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS anchors (
    chain TEXT PRIMARY KEY CHECK (chain IN ('long', 'short')),
    through_seq INTEGER NOT NULL,
    through_gseq INTEGER NOT NULL,
    through_entry_hash TEXT NOT NULL,
    disposal_gseq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    session TEXT PRIMARY KEY,
    started_gseq INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sign_ins (
    key TEXT PRIMARY KEY,
    gseq INTEGER NOT NULL
);";

/// D3: a trigger refuses every `UPDATE`.
pub const NO_UPDATE_TRIGGER: &str = "
CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'the activity record is append-only'); END;";

/// D3: a second refuses a `DELETE` of any row past its chain's anchor. D9
/// moves the anchor only in the transaction that disposes.
pub const NO_DELETE_TRIGGER: &str = "
CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
WHEN OLD.seq > COALESCE((SELECT through_seq FROM anchors WHERE chain = OLD.chain), 0)
BEGIN SELECT RAISE(ABORT, 'the activity record deletes only an anchored prefix'); END;";

/// A request's events as one unit of the append: the sign-in and session
/// start it may open (each written only when new, inside the transaction),
/// then its own events.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `(key, event)`: written only if `key` is not yet in `sign_ins`.
    pub sign_in: Option<(String, Event)>,
    /// An unconditional sign-in: an authentication exchange the daemon
    /// performed itself (D1), such as a ticket redeemed.
    pub exchange: Option<Event>,
    /// `(session id, event)`: written only if the id is not yet in `sessions`.
    pub session_start: Option<(String, Event)>,
    pub events: Vec<Event>,
}

impl Entry {
    pub fn of(event: Event) -> Self {
        Entry {
            sign_in: None,
            exchange: None,
            session_start: None,
            events: vec![event],
        }
    }
}

/// What one committed append wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Appended {
    pub events: u64,
    pub bytes: u64,
    pub last_gseq: u64,
}

/// A chain's position: its last `seq` and that event's `entry_hash`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub seq: u64,
    pub entry_hash: String,
}

pub struct ActivityDb {
    conn: sqlite::Connection,
}

impl ActivityDb {
    /// Open (creating, mode `0600`) the record at `path` for appending.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        create_private(path)?;
        let conn = sqlite::Connection::open(path)?;
        conn.execute(format!(
            "PRAGMA busy_timeout={BUSY_TIMEOUT_MS};
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;"
        ))?;
        conn.execute(format!("{SCHEMA}{NO_UPDATE_TRIGGER}{NO_DELETE_TRIGGER}"))?;
        Ok(ActivityDb { conn })
    }

    /// Open an existing record read-only (export, status, verify).
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        if !path.is_file() {
            return Err(StoreError::Io(format!("{} does not exist", path.display())));
        }
        let conn =
            sqlite::Connection::open_with_flags(path, sqlite::OpenFlags::new().with_read_only())?;
        conn.execute(format!("PRAGMA busy_timeout={BUSY_TIMEOUT_MS};"))?;
        Ok(ActivityDb { conn })
    }

    /// The connection, for tests that act as another process would.
    pub fn connection(&self) -> &sqlite::Connection {
        &self.conn
    }

    /// Append `entries` in one `BEGIN IMMEDIATE` transaction: all or none.
    pub fn append(&self, entries: &[Entry]) -> Result<Appended, StoreError> {
        self.append_with(|| entries.to_vec())
    }

    /// [`append`](Self::append) the entries `build` returns, called only once
    /// the write lock is held: what the writer appends is decided after any
    /// wait for the lock, not before it.
    pub fn append_with(&self, build: impl FnOnce() -> Vec<Entry>) -> Result<Appended, StoreError> {
        self.conn.execute("BEGIN IMMEDIATE")?;
        let entries = build();
        match self.append_in_tx(&entries) {
            Ok(done) => match self.conn.execute("COMMIT") {
                Ok(()) => Ok(done),
                Err(e) => {
                    let _ = self.conn.execute("ROLLBACK");
                    Err(e.into())
                }
            },
            Err(e) => {
                let _ = self.conn.execute("ROLLBACK");
                Err(e)
            }
        }
    }

    fn append_in_tx(&self, entries: &[Entry]) -> Result<Appended, StoreError> {
        let mut pos = Position::read(&self.conn)?;
        let mut done = Appended::default();
        for entry in entries {
            if let Some((key, ev)) = &entry.sign_in {
                if !self.exists("sign_ins", "key", key)? {
                    let gseq = self.write(&mut pos, ev, &mut done)?;
                    self.insert_mark("sign_ins", "key", key, gseq)?;
                }
            }
            if let Some(ev) = &entry.exchange {
                self.write(&mut pos, ev, &mut done)?;
            }
            if let Some((session, ev)) = &entry.session_start {
                if !self.exists("sessions", "session", session)? {
                    let gseq = self.write(&mut pos, ev, &mut done)?;
                    self.insert_mark("sessions", "session", session, gseq)?;
                }
            }
            for ev in &entry.events {
                self.write(&mut pos, ev, &mut done)?;
            }
        }
        done.last_gseq = pos.gseq;
        Ok(done)
    }

    /// Write `ev` at the next position, after a `clock_step` if its `at` is
    /// more than a second before the latest committed one (D10).
    fn write(
        &self,
        pos: &mut Position,
        ev: &Event,
        done: &mut Appended,
    ) -> Result<u64, StoreError> {
        if let Some(latest) = pos.latest_at_us {
            if ev.at_us + 1_000_000 < latest {
                let mut step = Event::daemon(EventKind::ClockStep, ev.at_us);
                step.detail = serde_json::json!({
                    "latest_at": super::at_string(latest),
                    "at": super::at_string(ev.at_us),
                });
                self.insert(pos, &step, done)?;
            }
        }
        self.insert(pos, ev, done)
    }

    fn insert(
        &self,
        pos: &mut Position,
        ev: &Event,
        done: &mut Appended,
    ) -> Result<u64, StoreError> {
        let chain = ev.chain();
        let gseq = pos.gseq + 1;
        let tip = pos.tip_mut(chain).clone();
        let sealed = seal(ev, &chain::fresh_salt(), tip.seq + 1, gseq, &tip.entry_hash)
            .map_err(|e| StoreError::Corrupt(format!("an event does not encode: {e}")))?;
        self.insert_sealed(&sealed)?;
        let tip = pos.tip_mut(chain);
        tip.seq = sealed.link.seq;
        tip.entry_hash = sealed.entry_hash.clone();
        pos.gseq = gseq;
        pos.latest_at_us = Some(pos.latest_at_us.map_or(ev.at_us, |l| l.max(ev.at_us)));
        done.events += 1;
        done.bytes += (sealed.link_bytes.len() + sealed.body_bytes.len()) as u64;
        Ok(gseq)
    }

    fn insert_sealed(&self, s: &Sealed) -> Result<(), StoreError> {
        let mut st = self.conn.prepare(
            "INSERT INTO events (gseq, chain, seq, link, body, at, principal_id, kb_id, kind, \
             outcome, outcome_kind, session, tenant) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        )?;
        let c = &s.columns;
        st.bind((1, s.link.gseq as i64))?;
        st.bind((2, s.link.chain.as_str()))?;
        st.bind((3, s.link.seq as i64))?;
        st.bind((4, &s.link_bytes[..]))?;
        st.bind((5, &s.body_bytes[..]))?;
        st.bind((6, c.at.as_str()))?;
        st.bind((7, c.principal_id.as_deref()))?;
        st.bind((8, c.kb_id.as_deref()))?;
        st.bind((9, c.kind.as_str()))?;
        st.bind((10, c.outcome.as_str()))?;
        st.bind((11, c.outcome_kind.as_deref()))?;
        st.bind((12, c.session.as_deref()))?;
        st.bind((13, c.tenant.as_deref()))?;
        st.next()?;
        Ok(())
    }

    fn exists(&self, table: &str, col: &str, key: &str) -> Result<bool, StoreError> {
        let mut st = self
            .conn
            .prepare(format!("SELECT 1 FROM {table} WHERE {col} = ?1"))?;
        st.bind((1, key))?;
        Ok(st.next()? == sqlite::State::Row)
    }

    fn insert_mark(&self, table: &str, col: &str, key: &str, gseq: u64) -> Result<(), StoreError> {
        let gcol = if table == "sessions" {
            "started_gseq"
        } else {
            "gseq"
        };
        let mut st = self.conn.prepare(format!(
            "INSERT INTO {table} ({col}, {gcol}) VALUES (?1, ?2)"
        ))?;
        st.bind((1, key))?;
        st.bind((2, gseq as i64))?;
        st.next()?;
        Ok(())
    }

    /// Each chain's tip, read now.
    pub fn tips(&self) -> Result<(Tip, Tip), StoreError> {
        let pos = Position::read(&self.conn)?;
        Ok((pos.long, pos.short))
    }

    /// The `at` of the oldest event, if any.
    pub fn oldest_at(&self) -> Result<Option<String>, StoreError> {
        let mut st = self.conn.prepare("SELECT min(at) AS at FROM events")?;
        st.next()?;
        Ok(st.read::<Option<String>, _>("at")?)
    }

    /// The body of the latest event of `kind`, if any.
    pub fn latest_body_of(&self, kind: EventKind) -> Result<Option<serde_json::Value>, StoreError> {
        let mut st = self
            .conn
            .prepare("SELECT body FROM events WHERE kind = ?1 ORDER BY gseq DESC LIMIT 1")?;
        st.bind((1, kind.as_str()))?;
        if st.next()? != sqlite::State::Row {
            return Ok(None);
        }
        let body: Vec<u8> = st.read("body")?;
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(|e| StoreError::Corrupt(e.to_string()))
    }

    /// Every stored row with `gseq` in `from..=to`, in `gseq` order, to `f`.
    pub fn for_each_row(
        &self,
        from: u64,
        to: u64,
        mut f: impl FnMut(StoredRow) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut st = self
            .conn
            .prepare(
                "SELECT gseq, chain, seq, link, body, at, principal_id, kb_id, kind, outcome, \
                 outcome_kind, session, tenant FROM events WHERE gseq BETWEEN ?1 AND ?2 \
                 ORDER BY gseq",
            )
            .map_err(|e| e.to_string())?;
        st.bind((1, from as i64)).map_err(|e| e.to_string())?;
        st.bind((2, to as i64)).map_err(|e| e.to_string())?;
        while st.next().map_err(|e| e.to_string())? == sqlite::State::Row {
            f(StoredRow::read(&st).map_err(|e| e.to_string())?)?;
        }
        Ok(())
    }

    /// The highest `gseq`, or 0.
    pub fn last_gseq(&self) -> Result<u64, StoreError> {
        Ok(Position::read(&self.conn)?.gseq)
    }

    /// The chain anchors (D9; none until P3 disposes).
    pub fn anchors(&self) -> Result<Vec<Anchor>, StoreError> {
        let mut st = self.conn.prepare(
            "SELECT chain, through_seq, through_gseq, through_entry_hash, disposal_gseq \
             FROM anchors ORDER BY chain",
        )?;
        let mut out = Vec::new();
        while st.next()? == sqlite::State::Row {
            let chain: String = st.read("chain")?;
            out.push(Anchor {
                chain: Chain::parse(&chain).ok_or_else(|| StoreError::Corrupt(chain.clone()))?,
                through_seq: st.read::<i64, _>("through_seq")? as u64,
                through_gseq: st.read::<i64, _>("through_gseq")? as u64,
                through_entry_hash: st.read("through_entry_hash")?,
                disposal_gseq: st.read::<i64, _>("disposal_gseq")? as u64,
            });
        }
        Ok(out)
    }
}

/// A chain's disposed prefix (D9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub chain: Chain,
    pub through_seq: u64,
    pub through_gseq: u64,
    pub through_entry_hash: String,
    pub disposal_gseq: u64,
}

/// One stored row, as read: the record's bytes and the index beside them.
#[derive(Debug, Clone)]
pub struct StoredRow {
    pub gseq: u64,
    pub chain: String,
    pub seq: u64,
    pub link: Vec<u8>,
    pub body: Vec<u8>,
    pub columns: Columns,
}

impl StoredRow {
    fn read(st: &sqlite::Statement<'_>) -> Result<StoredRow, sqlite::Error> {
        Ok(StoredRow {
            gseq: st.read::<i64, _>("gseq")? as u64,
            chain: st.read("chain")?,
            seq: st.read::<i64, _>("seq")? as u64,
            link: st.read("link")?,
            body: st.read("body")?,
            columns: Columns {
                at: st.read("at")?,
                principal_id: st.read("principal_id")?,
                kb_id: st.read("kb_id")?,
                kind: st.read("kind")?,
                outcome: st.read("outcome")?,
                outcome_kind: st.read("outcome_kind")?,
                session: st.read("session")?,
                tenant: st.read("tenant")?,
            },
        })
    }

    /// The row's columns all agree with its link and body (D3): `Err` names
    /// the first that does not.
    pub fn check_index(&self, link: &Link) -> Result<(), String> {
        if link.gseq != self.gseq || link.seq != self.seq || link.chain.as_str() != self.chain {
            return Err("its gseq, chain or seq column disagrees with its link".into());
        }
        let body: serde_json::Value =
            serde_json::from_slice(&self.body).map_err(|e| format!("body is not JSON: {e}"))?;
        match Columns::of(&body) {
            Some(c) if c == self.columns => Ok(()),
            _ => Err("an indexed column disagrees with its body".into()),
        }
    }
}

/// The append's view of the record, read inside its transaction.
struct Position {
    long: Tip,
    short: Tip,
    gseq: u64,
    latest_at_us: Option<i64>,
}

impl Position {
    fn read(conn: &sqlite::Connection) -> Result<Position, StoreError> {
        let mut st = conn.prepare("SELECT max(gseq) AS g, max(at) AS a FROM events")?;
        st.next()?;
        let gseq = st.read::<Option<i64>, _>("g")?.unwrap_or(0) as u64;
        let latest_at_us = st
            .read::<Option<String>, _>("a")?
            .and_then(|a| super::parse_at(&a));
        let anchored = |chain: Chain| -> Result<(Tip, u64), StoreError> {
            let mut st = conn.prepare(
                "SELECT through_seq, through_gseq, through_entry_hash FROM anchors WHERE chain = ?1",
            )?;
            st.bind((1, chain.as_str()))?;
            if st.next()? == sqlite::State::Row {
                return Ok((
                    Tip {
                        seq: st.read::<i64, _>("through_seq")? as u64,
                        entry_hash: st.read("through_entry_hash")?,
                    },
                    st.read::<i64, _>("through_gseq")? as u64,
                ));
            }
            Ok((
                Tip {
                    seq: 0,
                    entry_hash: ZERO_HASH.into(),
                },
                0,
            ))
        };
        let tip = |chain: Chain| -> Result<(Tip, u64), StoreError> {
            let mut st = conn.prepare(
                "SELECT seq, link FROM events WHERE chain = ?1 ORDER BY seq DESC LIMIT 1",
            )?;
            st.bind((1, chain.as_str()))?;
            if st.next()? == sqlite::State::Row {
                let link: Vec<u8> = st.read("link")?;
                return Ok((
                    Tip {
                        seq: st.read::<i64, _>("seq")? as u64,
                        entry_hash: chain::entry_hash(&link),
                    },
                    0,
                ));
            }
            anchored(chain)
        };
        let (long, lg) = tip(Chain::Long)?;
        let (short, sg) = tip(Chain::Short)?;
        Ok(Position {
            long,
            short,
            gseq: gseq.max(lg).max(sg),
            latest_at_us,
        })
    }

    fn tip_mut(&mut self, chain: Chain) -> &mut Tip {
        match chain {
            Chain::Long => &mut self.long,
            Chain::Short => &mut self.short,
        }
    }
}

/// Create `path` with mode `0600` if it does not exist, and hold an existing
/// file to the same mode (D3: owned by the daemon's user, readable by no one
/// else). SQLite gives its `-wal` and `-shm` files the database's mode.
fn create_private(path: &Path) -> Result<(), StoreError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| StoreError::Io(e.to_string()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| StoreError::Io(format!("{}: {e}", path.display())))?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| StoreError::Io(format!("{}: {e}", path.display())))?;
    }
    Ok(())
}
