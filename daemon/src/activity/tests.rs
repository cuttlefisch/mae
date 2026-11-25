//! ADR-120 P1a's record tests: encoding, chains, triggers, export, verify,
//! coalescing, the clock, and the writer's fail-closed path. The listener's
//! tests (completeness, the header, the 503) are `oauth_activity_tests.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::chain::{self, Link};
use super::coalesce::Coalescer;
use super::event::*;
use super::export::{self, Filter, VerifyError};
use super::session::{self, SessionKey};
use super::store::{ActivityDb, Entry};
use super::writer::{ActivityRecord, UNAVAILABLE_KIND};
use super::ActivitySettings;

/// A small seeded generator: the mixes are random but reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

const T0: i64 = 1_790_000_000_000_000;

fn peer() -> Option<Peer> {
    Some(Peer {
        addr: "203.0.113.7:4431".into(),
        forwarded: Some("198.51.100.2".into()),
    })
}

/// A random event with every optional field set, the address included.
fn random_event(rng: &mut Rng, at: i64) -> Event {
    let kind = rng.pick(&EventKind::ALL);
    let mut ev = Event::new(kind, at, Transport::Https);
    ev.result = rng.pick(&[
        OutcomeResult::Ok,
        OutcomeResult::Refused,
        OutcomeResult::Error,
    ]);
    ev.outcome_kind = (ev.result != OutcomeResult::Ok).then(|| "not_member".to_string());
    ev.principal = Principal {
        id: Some(format!("SHA256:p{}", rng.next() % 5)),
        kind: ActorKind::Person,
        iss: Some("https://idp.example".into()),
        sub: Some(format!("u{}", rng.next() % 5)),
        on_behalf_of: None,
        grant: None,
    };
    ev.session = Some(session::random_id());
    ev.route = Some("kb/query.get".into());
    ev.kb_id = Some(format!("kb{}", rng.next() % 3));
    ev.node_ids = Some(vec![format!("n{}", rng.next() % 9)]);
    ev.peer = peer();
    ev.unverified = Some(Unverified {
        iss: Some("claimed".into()),
        sub: Some("claimed".into()),
    });
    ev
}

fn open(dir: &tempfile::TempDir) -> ActivityDb {
    ActivityDb::open(&dir.path().join(super::DB_FILE)).unwrap()
}

/// Append `n` seeded events, one entry each.
fn fill(db: &ActivityDb, seed: u64, n: usize) {
    let mut rng = Rng(seed);
    for i in 0..n {
        db.append(&[Entry::of(random_event(&mut rng, T0 + i as i64))])
            .unwrap();
    }
}

fn bodies(db: &ActivityDb) -> Vec<Value> {
    let mut out = Vec::new();
    db.for_each_row(1, u64::MAX >> 2, |r| {
        out.push(serde_json::from_slice(&r.body).unwrap());
        Ok(())
    })
    .unwrap();
    out
}

// --- the address rule ------------------------------------------------------

/// Over a seeded random mix, no event outside `sign_in`, `session_start` and
/// refused outcomes carries a `peer`, whatever the caller set; the ones that
/// may, do. `unverified` survives only on a refusal.
#[test]
fn the_address_survives_only_where_the_rule_allows_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    fill(&db, 0x5eed_0001, 400);
    let mut allowed = 0;
    for b in bodies(&db) {
        let kind = b["kind"].as_str().unwrap();
        let refused = b["outcome"]["result"] == "refused";
        let may = refused || kind == "sign_in" || kind == "session_start";
        assert_eq!(!b["peer"].is_null(), may, "peer on {b}");
        assert_eq!(!b["unverified"].is_null(), refused, "unverified on {b}");
        allowed += usize::from(may);
    }
    assert!(allowed > 50 && allowed < 350, "the mix covered both sides");
}

/// The forwarded address is kept apart from the peer's own.
#[test]
fn a_forwarded_address_is_kept_apart_from_the_peers() {
    let mut ev = Event::new(EventKind::SignIn, T0, Transport::Https);
    ev.peer = peer();
    let b = ev.body(&[0; 16]);
    assert_eq!(b["peer"]["addr"], "203.0.113.7:4431");
    assert_eq!(b["peer"]["forwarded"], "198.51.100.2");
}

// --- classes ----------------------------------------------------------------

/// D7's table: only a successful request, read or subscription is short; a
/// stored successful read lands in `short`, and its refusal in `long`.
#[test]
fn only_successful_reads_are_short_lived() {
    for k in EventKind::ALL {
        for r in [
            OutcomeResult::Ok,
            OutcomeResult::Refused,
            OutcomeResult::Error,
        ] {
            let short = matches!(
                k,
                EventKind::Request | EventKind::Read | EventKind::Subscribe
            ) && r == OutcomeResult::Ok;
            assert_eq!(chain_of(k, r) == Chain::Short, short, "{k:?} {r:?}");
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    let read = Event::new(EventKind::Read, T0, Transport::Https);
    let mut refused = read.clone();
    refused.result = OutcomeResult::Refused;
    db.append(&[Entry::of(read), Entry::of(refused)]).unwrap();
    let mut chains = Vec::new();
    db.for_each_row(1, 2, |r| {
        chains.push(r.chain);
        Ok(())
    })
    .unwrap();
    assert_eq!(chains, ["short", "long"]);
}

// --- encoding ---------------------------------------------------------------

/// The salt enters the body hash: two bodies alike but for their salt hash
/// differently, so a stub confirms no guess at its content.
#[test]
fn the_salt_makes_the_body_hash_hiding() {
    let ev = Event::new(EventKind::Read, T0, Transport::Https);
    let a = chain::seal(&ev, &[1; 16], 1, 1, chain::ZERO_HASH).unwrap();
    let b = chain::seal(&ev, &[2; 16], 1, 1, chain::ZERO_HASH).unwrap();
    assert_ne!(a.link.body_hash, b.link.body_hash);
    let salted = chain::body_hash(&a.body_bytes);
    assert_eq!(salted, a.link.body_hash);
}

/// `prev_hash` and `gseq` are inside the bytes the entry hash covers.
#[test]
fn the_link_carries_its_position_and_predecessor() {
    let ev = Event::new(EventKind::Read, T0, Transport::Https);
    let a = chain::seal(&ev, &[1; 16], 1, 1, chain::ZERO_HASH).unwrap();
    let other_prev = "1".repeat(64);
    for (seq, gseq, prev) in [(1, 2, chain::ZERO_HASH), (1, 1, other_prev.as_str())] {
        let b = chain::seal(&ev, &[1; 16], seq, gseq, prev).unwrap();
        assert_ne!(a.entry_hash, b.entry_hash, "gseq {gseq} prev {prev}");
    }
    let parsed = Link::parse(&a.link_bytes).unwrap();
    assert_eq!(parsed, a.link);
}

#[test]
fn at_is_microseconds_and_reads_back() {
    for us in [0, 1, T0, T0 + 999_999, 951_782_400_000_001] {
        let s = super::at_string(us);
        assert_eq!(s.len(), 27, "{s}");
        assert_eq!(super::parse_at(&s), Some(us));
    }
    assert_eq!(
        super::at_string(951_782_400_000_001),
        "2000-02-29T00:00:00.000001Z"
    );
    assert_eq!(super::parse_at("2000-02-30T00:00:00.000000Z"), None);
}

// --- the stored chain -------------------------------------------------------

/// Flipping any byte of any stored link or body fails verification.
#[test]
fn flipping_any_byte_of_any_stored_link_or_body_fails_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::DB_FILE);
    let db = ActivityDb::open(&path).unwrap();
    fill(&db, 0x5eed_0002, 3);
    export::verify_db(&db).unwrap();
    let rows: Vec<(u64, Vec<u8>, Vec<u8>)> = {
        let mut v = Vec::new();
        db.for_each_row(1, 3, |r| {
            v.push((r.gseq, r.link, r.body));
            Ok(())
        })
        .unwrap();
        v
    };
    drop(db);
    for (gseq, link, body) in &rows {
        for (col, bytes) in [("link", link), ("body", body)] {
            for i in 0..bytes.len() {
                let mut flipped = bytes.clone();
                flipped[i] ^= 0x01;
                let copy = dir.path().join(format!("flip-{gseq}-{col}-{i}.db"));
                std::fs::copy(&path, &copy).unwrap();
                let c = sqlite::Connection::open(&copy).unwrap();
                c.execute("DROP TRIGGER events_no_update").unwrap();
                let mut st = c
                    .prepare(format!("UPDATE events SET {col} = ?1 WHERE gseq = ?2"))
                    .unwrap();
                st.bind((1, &flipped[..])).unwrap();
                st.bind((2, *gseq as i64)).unwrap();
                st.next().unwrap();
                drop(st);
                drop(c);
                let db = ActivityDb::open_read_only(&copy).unwrap();
                assert!(
                    export::verify_db(&db).is_err(),
                    "byte {i} of {col} at gseq {gseq} flipped and verify passed"
                );
                std::fs::remove_file(&copy).unwrap();
            }
        }
    }
}

/// Removing, swapping or reordering rows fails verification.
#[test]
fn removing_swapping_or_reordering_rows_fails_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::DB_FILE);
    fill(&ActivityDb::open(&path).unwrap(), 0x5eed_0003, 12);
    let tamper = |name: &str, sql: &str| {
        let copy = dir.path().join(format!("{name}.db"));
        std::fs::copy(&path, &copy).unwrap();
        let c = sqlite::Connection::open(&copy).unwrap();
        c.execute("DROP TRIGGER events_no_update; DROP TRIGGER events_no_delete;")
            .unwrap();
        c.execute(sql).unwrap();
        drop(c);
        let r = export::verify_db(&ActivityDb::open_read_only(&copy).unwrap());
        assert!(r.is_err(), "{name} passed verify");
    };
    // Not a chain's last row: until P2's heads, a removed tip is not seen.
    tamper("remove", "DELETE FROM events WHERE gseq = 3");
    tamper(
        "swap",
        "UPDATE events SET link = (SELECT link FROM events WHERE gseq = 5), \
         body = (SELECT body FROM events WHERE gseq = 5) WHERE gseq = 4",
    );
    tamper(
        "reorder",
        "UPDATE events SET gseq = gseq + 100 WHERE gseq = 2; \
         UPDATE events SET gseq = 2 WHERE gseq = 7; UPDATE events SET gseq = 7 WHERE gseq = 102",
    );
}

/// Editing an indexed column to hide a row from a filter fails verification,
/// and stops an export, naming the row.
#[test]
fn editing_an_indexed_column_fails_verify_and_export() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::DB_FILE);
    fill(&ActivityDb::open(&path).unwrap(), 0x5eed_0004, 5);
    for col in [
        "at",
        "principal_id",
        "kb_id",
        "kind",
        "outcome",
        "outcome_kind",
        "session",
        "tenant",
    ] {
        let copy = dir.path().join(format!("{col}.db"));
        std::fs::copy(&path, &copy).unwrap();
        let c = sqlite::Connection::open(&copy).unwrap();
        c.execute("DROP TRIGGER events_no_update").unwrap();
        c.execute(format!("UPDATE events SET {col} = 'hidden' WHERE gseq = 3"))
            .unwrap();
        drop(c);
        let db = ActivityDb::open_read_only(&copy).unwrap();
        match export::verify_db(&db) {
            Err(VerifyError::At { gseq: 3, .. }) => {}
            other => panic!("{col}: {other:?}"),
        }
        let err = export::export(&db, &Filter::default(), false, &mut Vec::new()).unwrap_err();
        assert!(err.starts_with("gseq 3:"), "{col}: {err}");
    }
}

/// D3's triggers: every `UPDATE` is refused, and so is a `DELETE` of a row
/// past its chain's anchor (all of them, with no anchor yet).
#[test]
fn the_triggers_refuse_update_and_unanchored_delete() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    fill(&db, 0x5eed_0005, 3);
    let c = db.connection();
    assert!(c
        .execute("UPDATE events SET kind = 'x' WHERE gseq = 1")
        .is_err());
    assert!(c.execute("DELETE FROM events WHERE gseq = 1").is_err());
    assert_eq!(bodies(&db).len(), 3);
    export::verify_db(&db).unwrap();
}

/// An operator command appending from another connection while the daemon's
/// writer appends leaves both chains linear, over seeded interleavings.
#[test]
fn concurrent_appenders_leave_both_chains_linear() {
    for seed in 1..=8u64 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(super::DB_FILE);
        let daemon = ActivityDb::open(&path).unwrap();
        let operator = ActivityDb::open(&path).unwrap();
        let mut rng = Rng(0x5eed_0100 + seed);
        let mut n = 0;
        for i in 0..40 {
            let who = if rng.next().is_multiple_of(2) {
                &daemon
            } else {
                &operator
            };
            let batch: Vec<Entry> = (0..=(rng.next() % 3))
                .map(|j| Entry::of(random_event(&mut rng, T0 + i * 10 + j as i64)))
                .collect();
            n += batch.len() as u64;
            who.append(&batch).unwrap();
        }
        let v = export::verify_db(&daemon).unwrap();
        assert_eq!(v.last_gseq, n);
        assert_eq!(v.long_seq + v.short_seq, n);
    }
}

// --- sessions and sign-ins --------------------------------------------------

/// A session start is written once per id, before the request's event; a
/// sign-in once per key.
#[test]
fn a_session_and_a_sign_in_start_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    let entry = |session: &str, at: i64| {
        let mut start = Event::new(EventKind::SessionStart, at, Transport::Https);
        start.session = Some(session.into());
        let sign_in = Event::new(EventKind::SignIn, at, Transport::Https);
        let mut read = Event::new(EventKind::Read, at, Transport::Https);
        read.session = Some(session.into());
        Entry {
            sign_in: Some(("k1".into(), sign_in)),
            exchange: None,
            session_start: Some((session.into(), start)),
            events: vec![read],
        }
    };
    let (a, b) = (session::random_id(), session::random_id());
    db.append(&[entry(&a, T0), entry(&a, T0 + 1)]).unwrap();
    db.append(&[entry(&b, T0 + 2)]).unwrap();
    let kinds: Vec<String> = bodies(&db)
        .iter()
        .map(|b| b["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        [
            "sign_in",
            "session_start",
            "read",
            "read",
            "session_start",
            "read"
        ]
    );
}

#[test]
fn session_ids_have_the_pinned_shape_and_follow_the_token() {
    let key = SessionKey::generate();
    let (a, a2, b) = (
        key.bearer_id("tok-a"),
        key.bearer_id("tok-a"),
        key.bearer_id("tok-b"),
    );
    assert_eq!(a, a2);
    assert_ne!(a, b);
    assert_ne!(a, SessionKey::generate().bearer_id("tok-a"), "keyed");
    for id in [a, b, session::random_id()] {
        assert!(session::is_session_id(&id), "{id}");
        assert!(!id.contains("tok"));
    }
    assert!(!session::is_session_id("ABCDEFGHIJKLMNOPQRSTUVWXYZ"));
}

/// The key is created once, mode 0600, and a wrong-length file is refused.
#[test]
fn the_session_key_is_created_private_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(super::SESSION_KEY_FILE);
    let k = SessionKey::load_or_create(&p).unwrap();
    let again = SessionKey::load_or_create(&p).unwrap();
    assert_eq!(k.bearer_id("t"), again.bearer_id("t"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let db = dir.path().join(super::DB_FILE);
        ActivityDb::open(&db).unwrap();
        let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    std::fs::write(&p, b"short").unwrap();
    assert!(SessionKey::load_or_create(&p).is_err());
}

// --- the clock --------------------------------------------------------------

/// A backward step of more than a second records `clock_step`, and `gseq`
/// order holds; a smaller inversion (within a batch) records nothing.
#[test]
fn a_backward_clock_step_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    let at = |us| Entry::of(Event::new(EventKind::Read, us, Transport::Https));
    db.append(&[at(T0), at(T0 - 500_000)]).unwrap();
    db.append(&[at(T0 - 5_000_000)]).unwrap();
    let b = bodies(&db);
    let kinds: Vec<&str> = b.iter().map(|b| b["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["read", "read", "clock_step", "read"]);
    assert_eq!(b[2]["detail"]["latest_at"], super::at_string(T0));
    assert_eq!(b[2]["detail"]["at"], super::at_string(T0 - 5_000_000));
    assert_eq!(export::verify_db(&db).unwrap().last_gseq, 4);
}

// --- export -----------------------------------------------------------------

fn export_bytes(db: &ActivityDb, f: &Filter, no_proof: bool) -> Vec<u8> {
    let mut out = Vec::new();
    export::export(db, f, no_proof, &mut out).unwrap();
    out
}

/// A principal filter returns only that principal's events, and the file
/// verifies; one matching nothing still verifies over its range.
#[test]
fn a_filtered_export_verifies_and_holds_only_what_matched() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    fill(&db, 0x5eed_0006, 60);
    for (filter, expect_some) in [
        (
            Filter {
                principal: Some("SHA256:p2".into()),
                ..Filter::default()
            },
            true,
        ),
        (
            Filter {
                kb: Some("no-such-kb".into()),
                ..Filter::default()
            },
            false,
        ),
    ] {
        let bytes = export_bytes(&db, &filter, false);
        let v = export::verify_export(&mut &bytes[..]).unwrap();
        assert_eq!(v.events + v.omitted, 60);
        assert_eq!(v.events > 0, expect_some);
        for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let l: Value = serde_json::from_slice(line).unwrap();
            if l["type"] == "event" {
                let body = data_encoding::BASE64
                    .decode(l["body"].as_str().unwrap().as_bytes())
                    .unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                assert!(filter.matches(&body));
                assert_eq!(body["principal"]["id"], "SHA256:p2");
            }
        }
    }
}

/// Every filter narrows to what it names, on the body, not the columns.
/// A filter on one field.
fn filter(field: &str, v: &str) -> Filter {
    let mut f = Filter::default();
    let slot = match field {
        "principal" => &mut f.principal,
        "kb" => &mut f.kb,
        "kind" => &mut f.kind,
        "outcome" => &mut f.outcome,
        "session" => &mut f.session,
        "since" => &mut f.since,
        _ => &mut f.until,
    };
    *slot = Some(v.to_string());
    f
}

#[test]
fn each_filter_matches_only_its_own_events() {
    let mut ev = Event::new(EventKind::Read, T0, Transport::Https);
    ev.principal.id = Some("p".into());
    ev.kb_id = Some("kb".into());
    ev.session = Some("s".into());
    ev.result = OutcomeResult::Refused;
    ev.outcome_kind = Some("not_member".into());
    let b = ev.body(&[0; 16]);
    let at = super::at_string(T0);
    let mut both = filter("since", &at);
    both.until = Some(at.clone());
    let yes = [
        filter("principal", "p"),
        filter("kb", "kb"),
        filter("kind", "read"),
        filter("outcome", "refused"),
        filter("outcome", "not_member"),
        filter("session", "s"),
        both,
    ];
    let no = [
        filter("principal", "q"),
        filter("kb", "other"),
        filter("kind", "request"),
        filter("outcome", "ok"),
        filter("outcome", "role_forbidden"),
        filter("session", "t"),
        filter("since", &super::at_string(T0 + 1)),
        filter("until", &super::at_string(T0 - 1)),
    ];
    for f in yes {
        assert!(f.matches(&b), "{f:?}");
    }
    for f in no {
        assert!(!f.matches(&b), "{f:?}");
    }
    assert_eq!(
        export::parse_time("2026-11-25"),
        Some("2026-11-25T00:00:00.000000Z".into())
    );
    assert_eq!(
        export::parse_time("2026-11-25T10:30:00Z"),
        Some("2026-11-25T10:30:00.000000Z".into())
    );
    assert_eq!(export::parse_time("yesterday"), None);
}

/// Altering an omitted event's stub, removing a line, or truncating the
/// file's tail fails verification; a `--no-proof` file is refused.
#[test]
fn an_altered_stub_a_missing_line_or_no_proof_fails_verify() {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    fill(&db, 0x5eed_0007, 20);
    let filter = Filter {
        principal: Some("SHA256:p1".into()),
        ..Filter::default()
    };
    let text = String::from_utf8(export_bytes(&db, &filter, false)).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let stub = lines
        .iter()
        .position(|l| l.contains("\"omitted\""))
        .unwrap();
    let v: Value = serde_json::from_str(lines[stub]).unwrap();
    let mut link = data_encoding::BASE64
        .decode(v["link"].as_str().unwrap().as_bytes())
        .unwrap();
    let at = link.len() - 5;
    link[at] = if link[at] == b'a' { b'b' } else { b'a' };
    let altered =
        serde_json::json!({"type": "omitted", "link": data_encoding::BASE64.encode(&link)});
    let mut changed = lines.clone();
    let altered = altered.to_string();
    changed[stub] = &altered;
    let joined = changed.join("\n");
    assert!(export::verify_export(&mut joined.as_bytes()).is_err());
    let mut dropped = lines.clone();
    dropped.remove(stub);
    assert!(export::verify_export(&mut dropped.join("\n").as_bytes()).is_err());
    let truncated = lines[..lines.len() - 1].join("\n");
    assert!(matches!(
        export::verify_export(&mut truncated.as_bytes()),
        Err(VerifyError::Missing { .. })
    ));
    let no_proof = export_bytes(&db, &filter, true);
    assert!(matches!(
        export::verify_export(&mut &no_proof[..]),
        Err(VerifyError::File(_))
    ));
    assert!(export::verify_export(&mut text.as_bytes()).is_ok());
}

// --- coalescing ---------------------------------------------------------------

fn bad_token(at: i64) -> Event {
    let mut ev = Event::new(EventKind::Request, at, Transport::Https);
    ev.result = OutcomeResult::Refused;
    ev.outcome_kind = Some("invalid_token".into());
    ev.route = Some("POST *".into());
    ev.peer = peer();
    ev
}

/// 10,000 bad-token refusals from one address write a bounded number of rows;
/// the first of each window is committed on its own, and at shutdown the
/// counts sum to 10,000.
#[test]
fn ten_thousand_refusals_coalesce_to_a_bounded_count_that_sums() {
    let settings = ActivitySettings {
        refusal_window_secs: 60,
        ..ActivitySettings::default()
    };
    let c = Coalescer::new(&settings);
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir);
    let mut committed_first = 0;
    for i in 0..10_000i64 {
        // Three windows: the clock crosses two window ends.
        let now = T0 + i * 15_000;
        let ev = bad_token(now);
        let admit = c.admit(&ev, now, false);
        for e in &admit.flushed {
            db.append(std::slice::from_ref(e)).unwrap();
        }
        if admit.commit {
            committed_first += 1;
            db.append(&[Entry::of(ev)]).unwrap();
        }
    }
    for e in c.drain_all(T0 + 10_000 * 15_000) {
        db.append(&[e]).unwrap();
    }
    let rows = bodies(&db);
    assert_eq!(committed_first, 3);
    assert!(rows.len() <= 6, "{} rows", rows.len());
    let total: u64 = rows.iter().map(|b| b["count"].as_u64().unwrap()).sum();
    assert_eq!(total, 10_000);
    assert!(rows.iter().all(|b| b["peer"]["addr"] == "203.0.113.7:4431"));
}

/// Past half the queue, or past the window bound, refusals go into one
/// aggregate per kind, with no address.
#[test]
fn a_spread_flood_is_aggregated_without_addresses() {
    let settings = ActivitySettings {
        queue_capacity: 8,
        ..ActivitySettings::default()
    };
    let c = Coalescer::new(&settings);
    let mut commits = 0;
    for i in 0..100 {
        let mut ev = bad_token(T0 + i);
        ev.peer = Some(Peer {
            addr: format!("203.0.113.{i}:1"),
            forwarded: None,
        });
        commits += usize::from(c.admit(&ev, T0 + i, i >= 50).commit);
    }
    assert_eq!(commits, 8, "only as many windows as the bound");
    assert_eq!(c.open_windows(), 8);
    let flushed = c.drain_all(T0 + 1000);
    let agg: Vec<&Event> = flushed
        .iter()
        .flat_map(|e| &e.events)
        .filter(|e| e.peer.is_none())
        .collect();
    assert_eq!(agg.len(), 1);
    assert_eq!(agg[0].count, 92);
    assert_eq!(agg[0].principal, Principal::anonymous());
}

// --- the writer ------------------------------------------------------------

fn settings(capacity: usize, timeout_ms: u64) -> ActivitySettings {
    ActivitySettings {
        enabled: true,
        queue_capacity: capacity,
        commit_timeout_ms: timeout_ms,
        ..ActivitySettings::default()
    }
}

fn read_entry() -> Entry {
    Entry::of(Event::new(
        EventKind::Read,
        super::now_us(),
        Transport::Https,
    ))
}

/// Holds the record's write lock from another connection, as an operator
/// command in another process would, until dropped.
struct Held(ActivityDb);

impl Held {
    fn new(path: &std::path::Path) -> Held {
        let db = ActivityDb::open(path).unwrap();
        db.connection().execute("BEGIN IMMEDIATE").unwrap();
        Held(db)
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.connection().execute("ROLLBACK");
    }
}

/// While the writer is held, a commit is refused at the timeout and nothing
/// is acknowledged early; once released, the next commit goes through, and
/// the refused one is stored as the refusal its caller received.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_writer_refuses_at_the_timeout_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::DB_FILE);
    let rec = ActivityRecord::start(&path, settings(4, 300), SessionKey::generate());
    rec.commit(read_entry()).await.unwrap();
    let held = Held::new(&path);
    let started = Instant::now();
    assert_eq!(
        rec.commit(read_entry()).await,
        Err(super::writer::Unavailable)
    );
    assert!(started.elapsed() >= Duration::from_millis(280));
    assert!(!rec.healthy());
    drop(held);
    rec.commit(read_entry()).await.unwrap();
    assert!(rec.healthy());
    let db = ActivityDb::open_read_only(&path).unwrap();
    let b = bodies(&db);
    assert_eq!(b.len(), 3, "{b:#?}");
    assert_eq!(b[1]["outcome"]["kind"], UNAVAILABLE_KIND);
    assert_eq!(b[2]["outcome"]["result"], "ok");
    assert_eq!(rec.status().unavailable_total, 1);
}

/// With capacity N and the writer held, request N+1 waits for space and is
/// refused at the timeout without ever entering the queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_queue_refuses_at_the_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::DB_FILE);
    let n = 3;
    let rec = ActivityRecord::start(&path, settings(n, 400), SessionKey::generate());
    let held = Held::new(&path);
    // One is taken into the stalled batch; N more fill the queue.
    let mut waiting = Vec::new();
    for _ in 0..=n {
        let rec = Arc::clone(&rec);
        waiting.push(tokio::spawn(async move { rec.commit(read_entry()).await }));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(rec.status().queue_depth, n + 1);
    let started = Instant::now();
    assert!(rec.commit(read_entry()).await.is_err());
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(380), "{waited:?}");
    for w in waiting {
        assert!(w.await.unwrap().is_err());
    }
    drop(held);
    rec.commit(read_entry()).await.unwrap();
}

/// A real I/O failure in the writer (here SQLite refusing every insert)
/// fails the health check, and the first commit after it clears returns it
/// to healthy with no restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_failure_fails_health_and_recovers_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(super::DB_FILE);
    let rec = ActivityRecord::start(&path, settings(8, 300), SessionKey::generate());
    rec.commit(read_entry()).await.unwrap();
    let saboteur = ActivityDb::open(&path).unwrap();
    saboteur
        .connection()
        .execute(
            "CREATE TRIGGER fail BEFORE INSERT ON events \
             BEGIN SELECT RAISE(ABORT, 'disk I/O error'); END;",
        )
        .unwrap();
    assert!(rec.commit(read_entry()).await.is_err());
    assert!(!rec.healthy());
    saboteur.connection().execute("DROP TRIGGER fail").unwrap();
    // The failed batch is retried by itself.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !rec.healthy() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(rec.healthy());
    rec.commit(read_entry()).await.unwrap();
}

/// A record whose file cannot be opened starts unhealthy and refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_cannot_open_starts_unhealthy() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("not-a-dir");
    std::fs::write(&blocked, b"").unwrap();
    let rec = ActivityRecord::start(
        &blocked.join(super::DB_FILE),
        settings(4, 200),
        SessionKey::generate(),
    );
    assert!(!rec.healthy());
    assert!(rec.commit(read_entry()).await.is_err());
}

#[test]
fn settings_floors_stop_startup() {
    assert!(ActivitySettings::default().problems().is_empty());
    let short = ActivitySettings {
        retention_short_days: 364,
        ..ActivitySettings::default()
    };
    assert_eq!(short.problems().len(), 1);
    let long = ActivitySettings {
        retention_long_days: 729,
        ..ActivitySettings::default()
    };
    assert_eq!(long.problems().len(), 1);
    let longer = ActivitySettings {
        retention_long_days: 1000,
        retention_short_days: 400,
        ..ActivitySettings::default()
    };
    assert!(longer.problems().is_empty());
    assert_ne!(longer.digest(), ActivitySettings::default().digest());
    let parsed: ActivitySettings =
        toml::from_str("enabled = true\ntrusted_proxies = [\"10.0.0.1\"]").unwrap();
    assert!(parsed.enabled);
    assert_eq!(parsed.queue_capacity, 4096);
    assert!(toml::from_str::<ActivitySettings>("enabeld = true").is_err());
}
