//! The daemon's activity record (ADR-120): one event per request at each of
//! the daemon's entry points, appended to its own SQLite file, chained, and
//! committed before the request is answered.
//!
//! - [`event`]: the body (D2), the two chains and the class function (D7), and
//!   the address rule, which the encoder applies whatever a caller set.
//! - [`chain`]: the link, the hiding body hash and the entry hash (D7).
//! - [`store`]: `activity.db`, its append-only triggers, and the one append
//!   transaction that reads each chain's tip inside itself (D3, D5).
//! - [`writer`]: the bounded queue, group commit, the acknowledgement every
//!   request awaits, health, and recovery (D5, D13).
//! - [`session`]: the daemon's session ids (D4).
//! - [`coalesce`]: refusals coalesced per window, never dropped (D6).
//! - [`export`]: the operator's export and the offline verifier (D8).
//!
//! P1a of the build sequence: the HTTPS listener only. Collab sessions, the
//! KB socket and reconciliation are P1b; signed heads are P2; disposal is P3.
//!
//! @ai-caution: [security] Principle #16: no agent reads or configures this
//! record. Nothing here is reachable from an editor tool, a Scheme primitive,
//! a command or `/mcp`; its settings exist only in `daemon.toml`.

pub mod chain;
pub mod coalesce;
pub mod event;
pub mod export;
pub mod session;
pub mod store;
pub mod writer;

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The record's file, beside `state.db` in the collab data directory (D3).
pub const DB_FILE: &str = "activity.db";
/// The session-id key (D4): 32 random bytes, mode `0600`.
pub const SESSION_KEY_FILE: &str = "activity-session.key";
/// D9's floors. Configuration may lengthen a period, never shorten it.
pub const RETENTION_LONG_FLOOR_DAYS: u32 = 730;
pub const RETENTION_SHORT_FLOOR_DAYS: u32 = 365;
/// P3's deadline check (ADR-120 "P3. Disposal"): until disposal ships, startup
/// logs an error once the oldest event is this old.
pub const DISPOSAL_WARNING_DAYS: i64 = 330;

/// `[activity]` in `daemon.toml` (D11). Every default is D11's table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActivitySettings {
    pub enabled: bool,
    pub queue_capacity: usize,
    pub commit_timeout_ms: u64,
    pub head_interval_secs: u64,
    pub span_close_secs: u64,
    pub refusal_window_secs: u64,
    pub retention_long_days: u32,
    pub retention_short_days: u32,
    /// Peers whose `X-Forwarded-For` is recorded, as `peer.forwarded` (D2).
    pub trusted_proxies: Vec<IpAddr>,
}

impl Default for ActivitySettings {
    fn default() -> Self {
        ActivitySettings {
            enabled: false,
            queue_capacity: 4096,
            commit_timeout_ms: 2000,
            head_interval_secs: 10,
            span_close_secs: 60,
            refusal_window_secs: 60,
            retention_long_days: RETENTION_LONG_FLOOR_DAYS,
            retention_short_days: RETENTION_SHORT_FLOOR_DAYS,
            trusted_proxies: Vec::new(),
        }
    }
}

impl ActivitySettings {
    /// What stops the daemon at startup (D9: a period under its floor is
    /// deleting early by another name; a zero capacity or timeout is no
    /// record at all). Empty when the settings may run.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.retention_long_days < RETENTION_LONG_FLOOR_DAYS {
            out.push(format!(
                "activity.retention_long_days = {} is under its floor of {RETENTION_LONG_FLOOR_DAYS}",
                self.retention_long_days
            ));
        }
        if self.retention_short_days < RETENTION_SHORT_FLOOR_DAYS {
            out.push(format!(
                "activity.retention_short_days = {} is under its floor of {RETENTION_SHORT_FLOOR_DAYS}",
                self.retention_short_days
            ));
        }
        for (name, v) in [
            ("queue_capacity", self.queue_capacity as u64),
            ("commit_timeout_ms", self.commit_timeout_ms),
            ("refusal_window_secs", self.refusal_window_secs),
        ] {
            if v == 0 {
                out.push(format!("activity.{name} must be at least 1"));
            }
        }
        out
    }

    /// The retention periods, as `daemon_start` and `config_changed` state them.
    pub fn retention(&self) -> serde_json::Value {
        serde_json::json!({
            "long_days": self.retention_long_days,
            "short_days": self.retention_short_days,
        })
    }

    /// SHA-256 of the canonical JSON of these settings, lowercase hex: the
    /// configuration digest `daemon_start` carries (D10).
    pub fn digest(&self) -> String {
        let v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        let bytes = crate::review::chain::canonical(&v).unwrap_or_default();
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(&bytes))
    }
}

/// Unix microseconds now, from the host clock (D10).
pub fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// `us` as RFC 3339 UTC with exactly six fractional digits (D2's `at`).
pub fn at_string(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let base = crate::rfc3339::utc(secs);
    format!(
        "{}.{:06}Z",
        &base[..base.len() - 1],
        us.rem_euclid(1_000_000)
    )
}

/// [`at_string`] read back; `None` for anything it would not have written.
pub fn parse_at(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 27 || &b[4..5] != b"-" || &b[19..20] != b"." || &b[26..27] != b"Z" {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())
            .flatten()
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se, us) = (num(11..13)?, num(14..16)?, num(17..19)?, num(20..26)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 59 {
        return None;
    }
    let days = days_from_civil(y, mo as u32, d as u32);
    let v = ((days * 86_400 + h * 3600 + mi * 60 + se) * 1_000_000) + us;
    (at_string(v) == s).then_some(v)
}

/// Howard Hinnant's `days_from_civil`, the inverse of `rfc3339`'s.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests;
