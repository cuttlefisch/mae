//! The operator's export and the offline verifier (ADR-120 D8).
//!
//! An export covers the whole stored range in `gseq` order: an `event` line
//! (link and body) for each event the filter matches, an `omitted` line (the
//! link only) for each it does not. The stubs let a verifier recompute every
//! `entry_hash` and see that nothing is missing, while the omitted bodies stay
//! out of the file. `--no-proof` leaves the stubs out: a smaller file for a
//! person reviewing patterns, which `verify` refuses as unverifiable.
//!
//! Signed heads are P2: until then `verify` checks the chains only, and what
//! it proves is limited accordingly — someone able to rewrite the file and
//! recompute the hashes is not detected, nor is a truncated tip of a chain
//! read straight from the database.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};

use super::chain::{self, Link, ZERO_HASH};
use super::event::{chain_of, Chain, EventKind, OutcomeResult};
use super::store::{ActivityDb, Anchor, StoredRow};

pub const EXPORT_FORMAT: &str = "maeactivity-export/v1";

/// D8's filters. Every one given must match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub principal: Option<String>,
    pub kb: Option<String>,
    pub kind: Option<String>,
    /// `ok`, `refused`, `error`, or a refusal kind.
    pub outcome: Option<String>,
    pub session: Option<String>,
    /// Inclusive bounds, in the body's own `at` form.
    pub since: Option<String>,
    pub until: Option<String>,
}

impl Filter {
    pub fn to_value(&self) -> Value {
        json!({
            "principal": self.principal, "kb": self.kb, "kind": self.kind,
            "outcome": self.outcome, "session": self.session,
            "since": self.since, "until": self.until,
        })
    }

    /// Whether `body` matches every filter given.
    pub fn matches(&self, body: &Value) -> bool {
        let field = |v: &Value| v.as_str().map(str::to_string);
        let eq = |want: &Option<String>, got: Option<String>| {
            want.as_ref()
                .is_none_or(|w| got.as_deref() == Some(w.as_str()))
        };
        let at = field(&body["at"]).unwrap_or_default();
        let outcome_ok = self.outcome.as_ref().is_none_or(|o| {
            let result = body["outcome"]["result"].as_str();
            match o.as_str() {
                "ok" | "refused" | "error" => result == Some(o.as_str()),
                kind => body["outcome"]["kind"].as_str() == Some(kind),
            }
        });
        eq(&self.principal, field(&body["principal"]["id"]))
            && eq(&self.kb, field(&body["kb_id"]))
            && eq(&self.kind, field(&body["kind"]))
            && eq(&self.session, field(&body["session"]))
            && outcome_ok
            && self
                .since
                .as_ref()
                .is_none_or(|s| at.as_str() >= s.as_str())
            && self
                .until
                .as_ref()
                .is_none_or(|u| at.as_str() <= u.as_str())
    }
}

/// A time given on the command line, in the body's own form: RFC 3339 UTC
/// with six fractional digits, or with none, or a bare date (its midnight).
pub fn parse_time(s: &str) -> Option<String> {
    let full = match s.len() {
        10 => format!("{s}T00:00:00.000000Z"),
        20 if s.ends_with('Z') => format!("{}.000000Z", &s[..19]),
        _ => s.to_string(),
    };
    super::parse_at(&full).map(super::at_string)
}

/// What an export wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Written {
    pub events: u64,
    pub omitted: u64,
    pub from_gseq: u64,
    pub to_gseq: u64,
}

/// Write the export of `db` under `filter` to `out`. Every row's indexed
/// columns are checked against its body first (D3): a column edited to hide a
/// row from a filter stops the export, naming the row.
pub fn export(
    db: &ActivityDb,
    filter: &Filter,
    no_proof: bool,
    out: &mut dyn Write,
) -> Result<Written, String> {
    let to = db.last_gseq().map_err(|e| e.to_string())?;
    let anchors = db.anchors().map_err(|e| e.to_string())?;
    let from = anchors.iter().map(|a| a.through_gseq).min().unwrap_or(0) + 1;
    let mut w = Written {
        from_gseq: from,
        to_gseq: to,
        ..Written::default()
    };
    let line = |v: Value, out: &mut dyn Write| -> Result<(), String> {
        writeln!(out, "{v}").map_err(|e| e.to_string())
    };
    line(
        json!({
            "type": "header", "format": EXPORT_FORMAT,
            // The daemon's key arrives with signed heads (P2). It is a claim
            // either way: `verify` takes the key from outside the file.
            "daemon_key": null,
            "filter": filter.to_value(), "no_proof": no_proof,
            "from_gseq": from, "to_gseq": to,
        }),
        out,
    )?;
    if !no_proof {
        for a in &anchors {
            line(anchor_line(a), out)?;
        }
    }
    let b64 = |b: &[u8]| data_encoding::BASE64.encode(b);
    db.for_each_row(from, to, |row: StoredRow| {
        let link = Link::parse(&row.link).map_err(|e| format!("gseq {}: {e}", row.gseq))?;
        row.check_index(&link)
            .map_err(|e| format!("gseq {}: {e}", row.gseq))?;
        let body: Value = serde_json::from_slice(&row.body).map_err(|e| e.to_string())?;
        if filter.matches(&body) {
            w.events += 1;
            line(
                json!({"type": "event", "link": b64(&row.link), "body": b64(&row.body)}),
                out,
            )
        } else if !no_proof {
            w.omitted += 1;
            line(json!({"type": "omitted", "link": b64(&row.link)}), out)
        } else {
            Ok(())
        }
    })?;
    Ok(w)
}

fn anchor_line(a: &Anchor) -> Value {
    json!({
        "type": "anchor", "chain": a.chain.as_str(), "through_seq": a.through_seq,
        "through_gseq": a.through_gseq, "through_entry_hash": a.through_entry_hash,
        "disposal_gseq": a.disposal_gseq,
    })
}

/// The first thing wrong, and where (D8: a failure names the first bad
/// `gseq`; a missing range is reported as a range).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    At { gseq: u64, why: String },
    Missing { from: u64, to: u64 },
    File(String),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::At { gseq, why } => write!(f, "gseq {gseq}: {why}"),
            VerifyError::Missing { from, to } => write!(f, "gseq {from}..={to} are missing"),
            VerifyError::File(why) => f.write_str(why),
        }
    }
}

/// What a successful verification covered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verified {
    pub events: u64,
    pub omitted: u64,
    pub last_gseq: u64,
    pub long_seq: u64,
    pub short_seq: u64,
}

/// One item to verify, in `gseq` order: a link, and the body when present.
pub struct Item<'a> {
    pub link: &'a [u8],
    pub body: Option<&'a [u8]>,
}

/// The chain verifier both inputs share (a file's lines, a database's rows).
pub struct Verifier {
    anchors: Vec<Anchor>,
    tips: [(u64, String); 2],
    last_gseq: u64,
    floor: u64,
    done: Verified,
}

impl Verifier {
    pub fn new(anchors: Vec<Anchor>) -> Self {
        let start = |c: Chain| {
            anchors
                .iter()
                .find(|a| a.chain == c)
                .map_or((0, ZERO_HASH.to_string()), |a| {
                    (a.through_seq, a.through_entry_hash.clone())
                })
        };
        let floor = anchors.iter().map(|a| a.through_gseq).max().unwrap_or(0);
        Verifier {
            tips: [start(Chain::Long), start(Chain::Short)],
            last_gseq: anchors.iter().map(|a| a.through_gseq).min().unwrap_or(0),
            floor,
            anchors,
            done: Verified::default(),
        }
    }

    fn slot(c: Chain) -> usize {
        match c {
            Chain::Long => 0,
            Chain::Short => 1,
        }
    }

    /// Check the next item.
    pub fn push(&mut self, item: Item<'_>) -> Result<Link, VerifyError> {
        let link = Link::parse(item.link).map_err(|why| VerifyError::At {
            gseq: self.last_gseq + 1,
            why: format!("a link does not parse: {why}"),
        })?;
        let at = |why: String| VerifyError::At {
            gseq: link.gseq,
            why,
        };
        if link.gseq <= self.last_gseq {
            return Err(at(format!(
                "gseq does not increase (after {})",
                self.last_gseq
            )));
        }
        if link.gseq > self.last_gseq + 1 && link.gseq - 1 > self.floor {
            return Err(VerifyError::Missing {
                from: (self.last_gseq + 1).max(self.floor + 1),
                to: link.gseq - 1,
            });
        }
        let (seq, prev) = &mut self.tips[Self::slot(link.chain)];
        if link.seq != *seq + 1 {
            return Err(at(format!(
                "{} chain: seq {} follows {}",
                link.chain.as_str(),
                link.seq,
                seq
            )));
        }
        if &link.prev_hash != prev {
            return Err(at("prev_hash is not the previous entry_hash".into()));
        }
        if let Some(body) = item.body {
            check_body(&link, body).map_err(at)?;
            self.done.events += 1;
        } else {
            self.done.omitted += 1;
        }
        *seq = link.seq;
        *prev = chain::entry_hash(item.link);
        self.last_gseq = link.gseq;
        Ok(link)
    }

    /// Finish, with the last `gseq` the input says it covers, if it says.
    pub fn finish(mut self, to_gseq: Option<u64>) -> Result<Verified, VerifyError> {
        if let Some(to) = to_gseq {
            if to > self.last_gseq {
                return Err(VerifyError::Missing {
                    from: self.last_gseq + 1,
                    to,
                });
            }
        }
        let _ = &self.anchors;
        self.done.last_gseq = self.last_gseq;
        self.done.long_seq = self.tips[0].0;
        self.done.short_seq = self.tips[1].0;
        Ok(self.done)
    }
}

/// The body's hash is the link's, and its kind and outcome class it into the
/// chain the link names.
fn check_body(link: &Link, body: &[u8]) -> Result<(), String> {
    if chain::body_hash(body) != link.body_hash {
        return Err("body_hash does not match the body".into());
    }
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("body is not JSON: {e}"))?;
    let kind = v["kind"]
        .as_str()
        .and_then(EventKind::parse)
        .ok_or("the body's kind is unknown")?;
    let result = match v["outcome"]["result"].as_str() {
        Some("ok") => OutcomeResult::Ok,
        Some("refused") => OutcomeResult::Refused,
        Some("error") => OutcomeResult::Error,
        _ => return Err("the body's outcome is unknown".into()),
    };
    if chain_of(kind, result) != link.chain {
        return Err("the event is not in the chain its kind and outcome class it into".into());
    }
    Ok(())
}

/// Verify the stored record itself: every row's chain position, its body,
/// and its indexed columns against its body (D3).
pub fn verify_db(db: &ActivityDb) -> Result<Verified, VerifyError> {
    let anchors = db.anchors().map_err(|e| VerifyError::File(e.to_string()))?;
    let to = db
        .last_gseq()
        .map_err(|e| VerifyError::File(e.to_string()))?;
    let from = anchors.iter().map(|a| a.through_gseq).min().unwrap_or(0) + 1;
    let mut v = Verifier::new(anchors);
    let mut failure = None;
    let walked = db.for_each_row(from, to, |row| {
        let pushed = v.push(Item {
            link: &row.link,
            body: Some(&row.body),
        });
        let checked = pushed.and_then(|link| {
            row.check_index(&link).map_err(|why| VerifyError::At {
                gseq: row.gseq,
                why,
            })
        });
        checked.map_err(|e| {
            failure = Some(e);
            String::new()
        })
    });
    if let Some(e) = failure {
        return Err(e);
    }
    walked.map_err(VerifyError::File)?;
    v.finish(Some(to))
}

/// Verify an export read from `input`, with no daemon configuration or
/// database (D8). A `--no-proof` export is refused as unverifiable.
pub fn verify_export(input: &mut dyn BufRead) -> Result<Verified, VerifyError> {
    let file = |why: String| VerifyError::File(why);
    let mut lines = input.lines();
    let header: Value = match lines.next() {
        Some(Ok(l)) => serde_json::from_str(&l).map_err(|e| file(format!("header: {e}")))?,
        _ => return Err(file("the file is empty".into())),
    };
    if header["type"] != "header" || header["format"] != EXPORT_FORMAT {
        return Err(file(format!("not a {EXPORT_FORMAT} export")));
    }
    if header["no_proof"] != json!(false) {
        return Err(file(
            "a --no-proof export carries no stubs and cannot be verified".into(),
        ));
    }
    let to = header["to_gseq"].as_u64();
    let b64 = |v: &Value| -> Result<Vec<u8>, VerifyError> {
        let s = v
            .as_str()
            .ok_or_else(|| file("a link or body is not a string".into()))?;
        data_encoding::BASE64
            .decode(s.as_bytes())
            .map_err(|e| file(format!("bad base64: {e}")))
    };
    let mut anchors = Vec::new();
    let mut verifier: Option<Verifier> = None;
    for l in lines {
        let l = l.map_err(|e| file(e.to_string()))?;
        let v: Value = serde_json::from_str(&l).map_err(|e| file(format!("a line: {e}")))?;
        match v["type"].as_str() {
            Some("anchor") if verifier.is_none() => anchors.push(parse_anchor(&v)?),
            Some("event") | Some("omitted") => {
                let verifier = verifier.get_or_insert_with(|| Verifier::new(anchors.clone()));
                let link = b64(&v["link"])?;
                let body = match v["type"] == "event" {
                    true => Some(b64(&v["body"])?),
                    false => None,
                };
                verifier.push(Item {
                    link: &link,
                    body: body.as_deref(),
                })?;
            }
            // Signed heads arrive with P2; a file that holds one is newer.
            Some("head") => return Err(file("this export holds signed heads (P2)".into())),
            _ => return Err(file(format!("an unknown line: {l}"))),
        }
    }
    verifier
        .unwrap_or_else(|| Verifier::new(anchors))
        .finish(to)
}

fn parse_anchor(v: &Value) -> Result<Anchor, VerifyError> {
    let bad = || VerifyError::File("a malformed anchor line".into());
    Ok(Anchor {
        chain: v["chain"].as_str().and_then(Chain::parse).ok_or_else(bad)?,
        through_seq: v["through_seq"].as_u64().ok_or_else(bad)?,
        through_gseq: v["through_gseq"].as_u64().ok_or_else(bad)?,
        through_entry_hash: v["through_entry_hash"]
            .as_str()
            .ok_or_else(bad)?
            .to_string(),
        disposal_gseq: v["disposal_gseq"].as_u64().ok_or_else(bad)?,
    })
}

/// Export to a new file at `path`, mode `0600`, never over an existing one.
pub fn export_to_file(
    db: &ActivityDb,
    filter: &Filter,
    no_proof: bool,
    path: &Path,
) -> Result<Written, String> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let f = opts
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = std::io::BufWriter::new(f);
    let written = export(db, filter, no_proof, &mut out)?;
    out.flush().map_err(|e| e.to_string())?;
    out.get_ref().sync_all().map_err(|e| e.to_string())?;
    Ok(written)
}
