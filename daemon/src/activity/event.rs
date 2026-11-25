//! An event's body (ADR-120 D2), its chain (D7), and the address rule.
//!
//! @ai-caution: [security] The address rule is a property of [`Event::body`],
//! not of its callers: `peer` survives only on a `sign_in`, a `session_start`
//! or a refused outcome, and `unverified` only on a refusal, whatever a
//! caller set. A client's address is personal data (the requirement's point
//! 2); every other event carries the session id instead.

use serde_json::{json, Value};

/// The event kinds (D2). Each is classed into a chain by [`chain_of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventKind {
    Request,
    Read,
    Subscribe,
    WriteSpanOpen,
    WriteSpanClose,
    WriteReconciled,
    UploadIntent,
    Upload,
    Download,
    ReviewAct,
    Admin,
    SignIn,
    SessionStart,
    SessionEnd,
    Disposal,
    DaemonStart,
    DaemonStop,
    RecordDisabled,
    ConfigChanged,
    ClockStep,
    HeadAttest,
}

impl EventKind {
    /// Every kind, for the tests that walk them.
    pub const ALL: [EventKind; 21] = [
        EventKind::Request,
        EventKind::Read,
        EventKind::Subscribe,
        EventKind::WriteSpanOpen,
        EventKind::WriteSpanClose,
        EventKind::WriteReconciled,
        EventKind::UploadIntent,
        EventKind::Upload,
        EventKind::Download,
        EventKind::ReviewAct,
        EventKind::Admin,
        EventKind::SignIn,
        EventKind::SessionStart,
        EventKind::SessionEnd,
        EventKind::Disposal,
        EventKind::DaemonStart,
        EventKind::DaemonStop,
        EventKind::RecordDisabled,
        EventKind::ConfigChanged,
        EventKind::ClockStep,
        EventKind::HeadAttest,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Request => "request",
            EventKind::Read => "read",
            EventKind::Subscribe => "subscribe",
            EventKind::WriteSpanOpen => "write_span_open",
            EventKind::WriteSpanClose => "write_span_close",
            EventKind::WriteReconciled => "write_reconciled",
            EventKind::UploadIntent => "upload_intent",
            EventKind::Upload => "upload",
            EventKind::Download => "download",
            EventKind::ReviewAct => "review_act",
            EventKind::Admin => "admin",
            EventKind::SignIn => "sign_in",
            EventKind::SessionStart => "session_start",
            EventKind::SessionEnd => "session_end",
            EventKind::Disposal => "disposal",
            EventKind::DaemonStart => "daemon_start",
            EventKind::DaemonStop => "daemon_stop",
            EventKind::RecordDisabled => "record_disabled",
            EventKind::ConfigChanged => "config_changed",
            EventKind::ClockStep => "clock_step",
            EventKind::HeadAttest => "head_attest",
        }
    }

    pub fn parse(s: &str) -> Option<EventKind> {
        EventKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// The two chains, one per retention class (D7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Chain {
    Long,
    Short,
}

impl Chain {
    pub const ALL: [Chain; 2] = [Chain::Long, Chain::Short];

    pub fn as_str(self) -> &'static str {
        match self {
            Chain::Long => "long",
            Chain::Short => "short",
        }
    }

    pub fn parse(s: &str) -> Option<Chain> {
        Chain::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// `outcome.result` (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutcomeResult {
    Ok,
    Refused,
    Error,
}

impl OutcomeResult {
    pub fn as_str(self) -> &'static str {
        match self {
            OutcomeResult::Ok => "ok",
            OutcomeResult::Refused => "refused",
            OutcomeResult::Error => "error",
        }
    }
}

/// D7's class table, as an exhaustive `match` with no default arm: a new kind
/// does not compile until it is classed. Only a SUCCESSFUL request, read or
/// subscription is short-lived; every refusal is long. An `error` outcome is
/// classed long too: D7 names it in neither row, and keeping a record longer
/// is the side that loses nothing.
pub fn chain_of(kind: EventKind, result: OutcomeResult) -> Chain {
    match kind {
        EventKind::Request | EventKind::Read | EventKind::Subscribe => match result {
            OutcomeResult::Ok => Chain::Short,
            OutcomeResult::Refused | OutcomeResult::Error => Chain::Long,
        },
        EventKind::WriteSpanOpen
        | EventKind::WriteSpanClose
        | EventKind::WriteReconciled
        | EventKind::UploadIntent
        | EventKind::Upload
        | EventKind::Download
        | EventKind::ReviewAct
        | EventKind::Admin
        | EventKind::SignIn
        | EventKind::SessionStart
        | EventKind::SessionEnd
        | EventKind::Disposal
        | EventKind::DaemonStart
        | EventKind::DaemonStop
        | EventKind::RecordDisabled
        | EventKind::ConfigChanged
        | EventKind::ClockStep
        | EventKind::HeadAttest => Chain::Long,
    }
}

/// `principal.kind` (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActorKind {
    Person,
    Agent,
    Service,
    Unbound,
    Fingerprint,
    Psk,
    Mesh,
    Local,
    Operator,
    Anonymous,
}

impl ActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ActorKind::Person => "person",
            ActorKind::Agent => "agent",
            ActorKind::Service => "service",
            ActorKind::Unbound => "unbound",
            ActorKind::Fingerprint => "fingerprint",
            ActorKind::Psk => "psk",
            ActorKind::Mesh => "mesh",
            ActorKind::Local => "local",
            ActorKind::Operator => "operator",
            ActorKind::Anonymous => "anonymous",
        }
    }
}

/// `principal` (D2). `on_behalf_of` and `grant` stay `None` until ADR-118's
/// agent tokens exist (P5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: Option<String>,
    pub kind: ActorKind,
    pub iss: Option<String>,
    pub sub: Option<String>,
    pub on_behalf_of: Option<String>,
    pub grant: Option<String>,
}

impl Principal {
    /// Nobody established: a refusal before authentication, or the daemon.
    pub fn anonymous() -> Self {
        Principal {
            id: None,
            kind: ActorKind::Anonymous,
            iss: None,
            sub: None,
            on_behalf_of: None,
            grant: None,
        }
    }

    /// `operator:<os user>`, the principal of a local command (D1).
    pub fn operator(os_user: &str) -> Self {
        Principal {
            id: Some(format!("operator:{os_user}")),
            kind: ActorKind::Operator,
            ..Principal::anonymous()
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "id": self.id, "kind": self.kind.as_str(), "iss": self.iss, "sub": self.sub,
            "on_behalf_of": self.on_behalf_of, "grant": self.grant,
        })
    }
}

/// `transport` (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Https,
    Ws,
    Mtls,
    Mesh,
    KbSocket,
    Cli,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Https => "https",
            Transport::Ws => "ws",
            Transport::Mtls => "mtls",
            Transport::Mesh => "mesh",
            Transport::KbSocket => "kb_socket",
            Transport::Cli => "cli",
        }
    }
}

/// `peer` (D2): where the request came from, and, only from a trusted proxy,
/// what it says it forwarded, kept apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub addr: String,
    pub forwarded: Option<String>,
}

/// `unverified` (D2): a refused token's claims, labelled, each truncated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unverified {
    pub iss: Option<String>,
    pub sub: Option<String>,
}

/// The bound D2 sets on an unverified claim, and the one this module applies
/// to every other string a client chose (a KB id, a node id).
pub const CLAIM_MAX_BYTES: usize = 256;

/// `s` cut to at most `max` bytes, on a character boundary.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// One event before it is sealed: everything but `v` and `salt`.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Microseconds, stamped when the daemon decides (D10).
    pub at_us: i64,
    pub kind: EventKind,
    pub principal: Principal,
    pub session: Option<String>,
    pub idp_sid: Option<String>,
    /// `None` for the daemon's own events (start, stop, a clock step).
    pub transport: Option<Transport>,
    pub route: Option<String>,
    pub kb_id: Option<String>,
    pub node_ids: Option<Vec<String>>,
    pub revision: Option<String>,
    pub blob: Option<String>,
    pub wal: Option<(i64, i64)>,
    pub result: OutcomeResult,
    pub outcome_kind: Option<String>,
    /// A coalesced refusal's count and window (D6); `1`, `at`, `at` otherwise.
    pub count: u64,
    pub first_at_us: i64,
    pub last_at_us: i64,
    pub peer: Option<Peer>,
    pub unverified: Option<Unverified>,
    pub reference: Option<Value>,
    pub detail: Value,
    /// The principal's ADR-060 tenant: an indexed column (D3), so it is in
    /// the body the column is derived from.
    pub tenant: Option<String>,
}

impl Event {
    /// An event of `kind` at `at_us` with every other field empty and an `ok`
    /// outcome: the base each entry point fills.
    pub fn new(kind: EventKind, at_us: i64, transport: Transport) -> Self {
        Event {
            transport: Some(transport),
            ..Event::daemon(kind, at_us)
        }
    }

    /// An event of the daemon's own, which arrived on no transport.
    pub fn daemon(kind: EventKind, at_us: i64) -> Self {
        Event {
            at_us,
            kind,
            principal: Principal::anonymous(),
            session: None,
            idp_sid: None,
            transport: None,
            route: None,
            kb_id: None,
            node_ids: None,
            revision: None,
            blob: None,
            wal: None,
            result: OutcomeResult::Ok,
            outcome_kind: None,
            count: 1,
            first_at_us: at_us,
            last_at_us: at_us,
            peer: None,
            unverified: None,
            reference: None,
            detail: Value::Null,
            tenant: None,
        }
    }

    pub fn chain(&self) -> Chain {
        chain_of(self.kind, self.result)
    }

    /// Whether this event may carry `peer` (the address rule).
    pub fn may_carry_address(&self) -> bool {
        matches!(self.kind, EventKind::SignIn | EventKind::SessionStart)
            || self.result == OutcomeResult::Refused
    }

    /// The body D2 defines, with `salt`, and with the address rule applied:
    /// `peer` and `unverified` are cleared wherever the rule does not allow
    /// them, whatever the caller put there.
    pub fn body(&self, salt: &[u8; 16]) -> Value {
        let peer = self
            .peer
            .as_ref()
            .filter(|_| self.may_carry_address())
            .map(|p| json!({"addr": p.addr, "forwarded": p.forwarded}));
        let unverified = self
            .unverified
            .as_ref()
            .filter(|_| self.result == OutcomeResult::Refused)
            .map(|u| json!({"iss": u.iss, "sub": u.sub}));
        let at = super::at_string(self.at_us);
        json!({
            "v": 1,
            "at": at,
            "kind": self.kind.as_str(),
            "principal": self.principal.to_value(),
            "session": self.session,
            "idp_sid": self.idp_sid,
            "transport": self.transport.map(Transport::as_str),
            "route": self.route,
            "kb_id": self.kb_id,
            "node_ids": self.node_ids,
            "revision": self.revision,
            "blob": self.blob,
            "wal": self.wal.map(|(from, to)| json!({"from": from, "to": to})),
            "outcome": {"result": self.result.as_str(), "kind": self.outcome_kind},
            "count": self.count,
            "first_at": super::at_string(self.first_at_us),
            "last_at": super::at_string(self.last_at_us),
            "peer": peer,
            "unverified": unverified,
            "ref": self.reference,
            "detail": self.detail,
            "tenant": self.tenant,
            "salt": data_encoding::BASE64.encode(salt),
        })
    }
}

/// The indexed columns D3 derives from a body: what the store writes beside
/// it and what verification recomputes. `None` for a body that is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Columns {
    pub at: String,
    pub principal_id: Option<String>,
    pub kb_id: Option<String>,
    pub kind: String,
    pub outcome: String,
    pub outcome_kind: Option<String>,
    pub session: Option<String>,
    pub tenant: Option<String>,
}

impl Columns {
    pub fn of(body: &Value) -> Option<Columns> {
        let s = |v: &Value| v.as_str().map(str::to_string);
        Some(Columns {
            at: s(body.get("at")?)?,
            principal_id: s(&body["principal"]["id"]),
            kb_id: s(&body["kb_id"]),
            kind: s(body.get("kind")?)?,
            outcome: s(&body["outcome"]["result"])?,
            outcome_kind: s(&body["outcome"]["kind"]),
            session: s(&body["session"]),
            tenant: s(&body["tenant"]),
        })
    }
}
