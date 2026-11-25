//! The activity record's place in the daemon's life (ADR-120 D10, D11) and
//! its operator commands (D8, D13):
//!
//! ```text
//! mae-daemon activity status [--json]
//! mae-daemon activity export [--principal ID] [--kb ID] [--kind K]
//!                            [--outcome ok|refused|error|KIND] [--session ID]
//!                            [--since T] [--until T] [--no-proof] --out FILE
//! mae-daemon activity verify [--in FILE]
//! ```
//!
//! `verify` without `--in` checks the stored record itself, indexed columns
//! included; with it, an export, with no daemon configuration or database.
//! Signed heads, `--key` and `--against` are P2.
//!
//! @ai-caution: [security] Principle #16: these are the only readers, and
//! they run on the host as the operator. No editor tool, Scheme primitive,
//! command or MCP method reaches them, and `daemon/activity_status` on the KB
//! socket reports counters and chain tips only, never an event.

use std::path::PathBuf;
use std::sync::Arc;

use mae_daemon::activity as act;
use mae_daemon::activity::event::{Event, EventKind, Principal, Transport};
use mae_daemon::activity::export::{self, Filter};
use mae_daemon::activity::session::{self, SessionKey};
use mae_daemon::activity::store::{ActivityDb, Entry};
use mae_daemon::activity::writer::{self, ActivityRecord};
use serde_json::{json, Value};

use crate::config::DaemonConfig;

/// `activity.db`'s path for this instance (D3: beside `state.db`).
pub(crate) fn db_path(config: &DaemonConfig) -> PathBuf {
    config.collab_data_dir().join(act::DB_FILE)
}

/// What `--check-config` reports about `[activity]`: problems stop the daemon.
pub(crate) fn check(config: &DaemonConfig) -> (Vec<String>, Vec<String>) {
    let a = &config.activity;
    let mut warnings = Vec::new();
    if config.oauth.enabled && !a.enabled {
        warnings.push(
            "the HTTPS listener is on and [activity] enabled is false: requests are not recorded"
                .to_string(),
        );
    }
    (a.problems(), warnings)
}

/// Start the record when enabled (D11). A setting under its floor stops the
/// daemon. With the record off and a record on disk, one `record_disabled`
/// event is appended and nothing more. `None` when the record is off.
pub(crate) async fn start(config: &DaemonConfig) -> Option<Arc<ActivityRecord>> {
    let settings = config.activity.clone();
    let path = db_path(config);
    if !settings.enabled {
        if path.is_file() {
            append_once(
                &path,
                Event::daemon(EventKind::RecordDisabled, act::now_us()),
            );
        }
        return None;
    }
    let problems = settings.problems();
    if !problems.is_empty() {
        for p in &problems {
            tracing::error!(problem = %p, "activity configuration error");
            eprintln!("Error: {p}");
        }
        std::process::exit(1);
    }
    let key_path = config.collab_data_dir().join(act::SESSION_KEY_FILE);
    let key = match SessionKey::load_or_create(&key_path) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("Error: activity session key: {e}");
            tracing::error!(error = %e, "activity session key unusable; not starting");
            std::process::exit(1);
        }
    };
    let previous = read_previous(&path);
    let record = ActivityRecord::start(&path, settings.clone(), key);
    let mut opening = vec![Entry::of(writer::daemon_start_event(
        &settings,
        crate::VERSION,
        record.status().tips.as_ref(),
    ))];
    if let Some(prev) = previous.retention.filter(|r| *r != settings.retention()) {
        let mut ev = Event::daemon(EventKind::ConfigChanged, act::now_us());
        ev.detail = json!({"retention": {"from": prev, "to": settings.retention()}});
        opening.push(Entry::of(ev));
    }
    // Queued, not awaited: a record that cannot open yet commits these once
    // it can, and never stores them as refused.
    for entry in opening {
        let _ = record.submit(entry).await;
    }
    if previous
        .oldest_days
        .is_some_and(|d| d >= act::DISPOSAL_WARNING_DAYS)
    {
        tracing::error!(
            days = previous.oldest_days,
            "activity record: the oldest event is near the end of its 365-day retention and \
             disposal (ADR-120 P3) is not built"
        );
    }
    spawn_coalesce_flush(Arc::clone(&record));
    tracing::info!(path = %path.display(), "activity record enabled");
    Some(record)
}

/// Clean shutdown (D10): flush the coalesced windows, then `daemon_stop`.
pub(crate) async fn stop(record: &ActivityRecord) {
    record
        .shutdown(Event::daemon(EventKind::DaemonStop, act::now_us()))
        .await;
}

/// D6: each window's count is queued as it ends, each sweep bounded by the
/// commit timeout (`ActivityRecord::flush_coalesced`).
fn spawn_coalesce_flush(record: Arc<ActivityRecord>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tick.tick().await;
            record.flush_coalesced(act::now_us()).await;
        }
    });
}

#[derive(Default)]
struct Previous {
    retention: Option<Value>,
    oldest_days: Option<i64>,
}

fn read_previous(path: &std::path::Path) -> Previous {
    let Ok(db) = ActivityDb::open_read_only(path) else {
        return Previous::default();
    };
    let retention = db
        .latest_body_of(EventKind::DaemonStart)
        .ok()
        .flatten()
        .map(|b| b["detail"]["retention"].clone());
    let oldest_days = db
        .oldest_at()
        .ok()
        .flatten()
        .and_then(|a| act::parse_at(&a))
        .map(|us| (act::now_us() - us) / 86_400_000_000);
    Previous {
        retention,
        oldest_days,
    }
}

fn append_once(path: &std::path::Path, ev: Event) {
    match ActivityDb::open(path).and_then(|db| db.append(&[Entry::of(ev)])) {
        Ok(_) => tracing::warn!("[activity] enabled is false: recorded record_disabled"),
        Err(e) => tracing::error!(error = %e, "could not record record_disabled"),
    }
}

const USAGE: &str = "usage: mae-daemon activity status [--json]
       mae-daemon activity export [--principal ID] [--kb ID] [--kind K] [--outcome ok|refused|error|KIND]
                                  [--session ID] [--since T] [--until T] [--no-proof] --out FILE
       mae-daemon activity verify [--in FILE]";

/// Whether this is `activity verify --in FILE`, which needs no configuration.
pub(crate) fn is_offline_verify(sub: Option<&str>, rest: &[String]) -> bool {
    sub == Some("activity")
        && rest.first().map(String::as_str) == Some("verify")
        && rest.iter().any(|a| a == "--in")
}

/// `mae-daemon activity …`. `config` is `None` only for an offline verify.
pub(crate) fn run(config: Option<&DaemonConfig>, rest: &[String]) -> i32 {
    let args = &rest[1.min(rest.len())..];
    match (rest.first().map(String::as_str), config) {
        (Some("verify"), _) if args.iter().any(|a| a == "--in") => verify_file(args),
        (Some("verify"), Some(c)) => verify_stored(c),
        (Some("status"), Some(c)) => status(c, args.iter().any(|a| a == "--json")),
        (Some("export"), Some(c)) => export_cmd(c, args),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

/// D13: exit 0 healthy, 1 unhealthy, 2 when the running daemon cannot be
/// reached. `--json` prints one object.
fn status(config: &DaemonConfig, as_json: bool) -> i32 {
    let mut client = mae_mcp::daemon_client::DaemonClient::new(&config.socket);
    client.set_timeout(std::time::Duration::from_secs(3));
    let answer = client.connect().map_err(|e| e.to_string()).and_then(|()| {
        client
            .call("daemon/activity_status", json!({}))
            .map_err(|e| e.to_string())
    });
    let s = match answer {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "activity status: the daemon is not answering on {}: {e}",
                config.socket.display()
            );
            return 2;
        }
    };
    let healthy = s["healthy"].as_bool() == Some(true);
    if as_json {
        println!("{s}");
    } else {
        let health = if healthy { "healthy" } else { "UNHEALTHY" };
        println!("activity record: {health} (enabled: {})", s["enabled"]);
        println!("  unavailable_total: {}", s["unavailable_total"]);
        println!("  queue_depth: {}", s["queue_depth"]);
        println!("  last_commit_age_ms: {}", s["last_commit_age_ms"]);
        for c in ["long", "short"] {
            println!(
                "  {c}: seq {} {}",
                s["chains"][c]["seq"], s["chains"][c]["entry_hash"]
            );
        }
    }
    i32::from(!healthy)
}

/// The `daemon/activity_status` answer (D13's fields). Head fields are null
/// until signed heads exist (P2).
pub(crate) fn status_value(record: Option<&ActivityRecord>) -> Value {
    let Some(record) = record else {
        return json!({
            "enabled": false, "healthy": true, "unavailable_total": 0, "queue_depth": 0,
            "last_commit_age_ms": null, "last_head_age_s": null,
            "chains": {"long": {"seq": null, "entry_hash": null},
                       "short": {"seq": null, "entry_hash": null}},
        });
    };
    let s = record.status();
    let tip = |t: Option<&mae_daemon::activity::store::Tip>| match t {
        Some(t) => json!({"seq": t.seq, "entry_hash": t.entry_hash}),
        None => json!({"seq": null, "entry_hash": null}),
    };
    json!({
        "enabled": true,
        "healthy": s.healthy,
        "unavailable_total": s.unavailable_total,
        "queue_depth": s.queue_depth,
        "last_commit_age_ms": s.last_commit_age_ms,
        "last_head_age_s": null,
        "chains": {
            "long": tip(s.tips.as_ref().map(|t| &t.0)),
            "short": tip(s.tips.as_ref().map(|t| &t.1)),
        },
    })
}

/// The OS user running this command (D1's `operator:<os user>`): the name
/// of the effective uid, or the uid itself.
fn os_user() -> String {
    #[cfg(unix)]
    {
        let uid = unsafe { libc::geteuid() };
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut out: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: getpwuid_r writes only into `pw` and `buf`, both owned here
        // and sized as passed; `out` is null or points at `pw`.
        let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
        if rc == 0 && !out.is_null() && !pw.pw_name.is_null() {
            // SAFETY: pw_name is a NUL-terminated string inside `buf`.
            let name = unsafe { std::ffi::CStr::from_ptr(pw.pw_name) };
            if let Ok(n) = name.to_str() {
                return n.to_string();
            }
        }
        format!("uid{uid}")
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERNAME").unwrap_or_else(|_| "unknown".into())
    }
}

/// D8: the export records its own run first, as a `read` by the operator,
/// then reads the record and writes the file.
fn export_cmd(config: &DaemonConfig, args: &[String]) -> i32 {
    let (filter, no_proof, out) = match parse_export(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("activity export: {e}\n{USAGE}");
            return 2;
        }
    };
    let path = db_path(config);
    if !path.is_file() {
        eprintln!("activity export: no record at {}", path.display());
        return 1;
    }
    let mut ev = Event::new(EventKind::Read, act::now_us(), Transport::Cli);
    ev.principal = Principal::operator(&os_user());
    ev.session = Some(session::random_id());
    ev.route = Some("activity export".into());
    ev.detail = json!({"filter": filter.to_value(), "no_proof": no_proof});
    if let Err(e) = ActivityDb::open(&path).and_then(|db| db.append(&[Entry::of(ev)])) {
        eprintln!("activity export: could not record this export, so it does not run: {e}");
        return 1;
    }
    let written = ActivityDb::open_read_only(&path)
        .map_err(|e| e.to_string())
        .and_then(|db| export::export_to_file(&db, &filter, no_proof, &out));
    match written {
        Ok(w) => {
            println!(
                "exported gseq {}..={}: {} event(s), {} omitted, to {}",
                w.from_gseq,
                w.to_gseq,
                w.events,
                w.omitted,
                out.display()
            );
            0
        }
        Err(e) => {
            eprintln!("activity export: {e}");
            1
        }
    }
}

fn parse_export(args: &[String]) -> Result<(Filter, bool, PathBuf), String> {
    let mut f = Filter::default();
    let mut no_proof = false;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--principal" => f.principal = Some(value()?),
            "--kb" => f.kb = Some(value()?),
            "--kind" => {
                let k = value()?;
                EventKind::parse(&k).ok_or(format!("unknown kind {k:?}"))?;
                f.kind = Some(k);
            }
            "--outcome" => f.outcome = Some(value()?),
            "--session" => f.session = Some(value()?),
            "--since" => {
                let t = value()?;
                f.since = Some(export::parse_time(&t).ok_or(format!("not a time: {t:?}"))?);
            }
            "--until" => {
                let t = value()?;
                f.until = Some(export::parse_time(&t).ok_or(format!("not a time: {t:?}"))?);
            }
            "--no-proof" => no_proof = true,
            "--out" => out = Some(PathBuf::from(value()?)),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok((f, no_proof, out.ok_or("--out FILE is required")?))
}

fn verify_file(args: &[String]) -> i32 {
    let Some(path) = args
        .iter()
        .position(|a| a == "--in")
        .and_then(|i| args.get(i + 1))
    else {
        eprintln!("{USAGE}");
        return 2;
    };
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("activity verify: {path}: {e}");
            return 1;
        }
    };
    report(export::verify_export(&mut std::io::BufReader::new(file)))
}

fn verify_stored(config: &DaemonConfig) -> i32 {
    match ActivityDb::open_read_only(&db_path(config)) {
        Ok(db) => report(export::verify_db(&db)),
        Err(e) => {
            eprintln!("activity verify: {e}");
            1
        }
    }
}

fn report(r: Result<export::Verified, export::VerifyError>) -> i32 {
    match r {
        Ok(v) => {
            println!(
                "OK: chains linear through gseq {} (long seq {}, short seq {}); {} event(s), {} \
                 stub(s). Chain only: signed heads are not built yet (ADR-120 P2).",
                v.last_gseq, v.long_seq, v.short_seq, v.events, v.omitted
            );
            0
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
            1
        }
    }
}
