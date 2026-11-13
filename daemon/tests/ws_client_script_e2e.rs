//! Conformance with the browser client's scripted conversation (ADR-099
//! Amendment 1, B1–B6).
//!
//! The web client is built against a mock that plays `c-guion-v1`: per
//! scenario, the frames and HTTP requests the client sends and exactly what it
//! expects back. This test replays every scenario against a real daemon over
//! the real TLS listener and checks each answer against the script's own
//! template, so the daemon and the mock cannot drift apart unnoticed.
//!
//! The script is vendored byte for byte (`tests/data/c-guion-v1.json`) and its
//! hash is pinned: a changed script fails here first, and the new version is
//! adopted deliberately. Its text is data: field names, values and placeholders
//! are read from it, nothing else.
//!
//! Template rules (`conforms`): an object must have exactly the template's keys
//! (no extras unless a scenario names them); `"<…>"` is any non-empty string;
//! the script's example values (a ticket, a principal, a timestamp) are
//! replaced by the live value or by a format check; everything else is
//! literal.

mod daemon_harness;
mod ws_common;

use std::collections::HashMap;
use std::time::Duration;

use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::{derive_kb_client_id, KbNodeDoc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ws_common::*;

const SCRIPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/c-guion-v1.json"
));
const SCRIPT_SHA256: &str = "1ec66e53ad049a651aa63f80c7442f66a76d3f9a1a86cf73788e7d1e93973df5";

/// The script's example ticket, replaced by a live one wherever it appears.
const EXAMPLE_TICKET: &str = "TICKET-43-caracteres-base64url-de-256-bits-x";
/// The script's example principal.
const EXAMPLE_PRINCIPAL: &str = "principal-de-prueba";
/// The script's KB and node ids, used as the live ids.
const KB: &str = "kb-a";

fn script() -> Value {
    let digest = Sha256::digest(SCRIPT.as_bytes());
    assert_eq!(
        hex::encode(digest),
        SCRIPT_SHA256,
        "tests/data/c-guion-v1.json is not the pinned copy of the client's script"
    );
    let v: Value = serde_json::from_str(SCRIPT).unwrap();
    assert_eq!(v["formato"], "c-guion-v1");
    v
}

fn scenario<'a>(script: &'a Value, name: &str) -> &'a Value {
    script["escenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["nombre"] == name)
        .unwrap_or_else(|| panic!("the script has no scenario '{name}'"))
}

/// Step `i` of a scenario, which must be a `side` ("cliente"/"demonio") step.
fn step<'a>(sc: &'a Value, i: usize, side: &str) -> &'a Value {
    let s = &sc["pasos"][i];
    assert!(
        s.get(side).is_some(),
        "step {i} of '{}' is not a '{side}' step: {s}",
        sc["nombre"]
    );
    &s[side]
}

// ---------------------------------------------------------------------------
// The template matcher
// ---------------------------------------------------------------------------

/// How a script example value is checked against the live one.
#[derive(Clone)]
enum Check {
    Eq(Value),
    /// RFC 3339, UTC, whole seconds — the shape of the script's examples.
    Rfc3339,
    /// 256 bits, base64url without padding: 43 characters.
    Ticket,
}

#[derive(Default, Clone)]
struct Subs(HashMap<String, Check>);

impl Subs {
    fn with(mut self, example: &str, check: Check) -> Self {
        self.0.insert(example.to_string(), check);
        self
    }
    /// The script's example timestamps are format checks.
    fn deadlines(self) -> Self {
        self.with("2026-11-11T12:00:00Z", Check::Rfc3339)
            .with("2026-11-11T13:00:00Z", Check::Rfc3339)
    }
}

fn check_string(want: &str, got: &Value, subs: &Subs, path: &str, errs: &mut Vec<String>) {
    let mut fail = |why: String| errs.push(format!("{path}: {why}, got {got}"));
    match subs.0.get(want) {
        Some(Check::Eq(v)) => {
            if got != v {
                fail(format!("expected {v}"));
            }
        }
        Some(Check::Rfc3339) => {
            let s = got.as_str().unwrap_or("");
            let utc_seconds = s.len() == 20 && s.ends_with('Z');
            if !utc_seconds || humantime::parse_rfc3339(s).is_err() {
                fail("expected RFC 3339 UTC with whole seconds".into());
            }
        }
        Some(Check::Ticket) => {
            let s = got.as_str().unwrap_or("");
            let bits = data_encoding::BASE64URL_NOPAD
                .decode(s.as_bytes())
                .map(|b| b.len() * 8)
                .unwrap_or(0);
            if s.len() != 43 || bits != 256 {
                fail("expected 256 bits as 43 base64url characters".into());
            }
        }
        None if want.starts_with('<') && want.ends_with('>') => {
            if got.as_str().is_none_or(str::is_empty) {
                fail(format!("expected a non-empty string for {want}"));
            }
        }
        None => {
            if got != &Value::String(want.to_string()) {
                fail(format!("expected \"{want}\""));
            }
        }
    }
}

/// Collect every way `got` differs from the template `want`. `extra` names
/// keys `got` may carry beyond the template's, anywhere.
fn conforms(
    want: &Value,
    got: &Value,
    subs: &Subs,
    extra: &[&str],
    path: &str,
    errs: &mut Vec<String>,
) {
    match want {
        Value::Object(w) => {
            let Some(g) = got.as_object() else {
                errs.push(format!("{path}: expected an object, got {got}"));
                return;
            };
            let mut keys = Vec::new();
            for (k, wv) in w {
                // A placeholder key (`"<clientID>"`) names a live key.
                let key = match subs.0.get(k) {
                    Some(Check::Eq(Value::String(s))) => s.clone(),
                    _ => k.clone(),
                };
                match g.get(&key) {
                    Some(gv) => conforms(wv, gv, subs, extra, &format!("{path}.{key}"), errs),
                    None => errs.push(format!("{path}: missing key '{key}' in {got}")),
                }
                keys.push(key);
            }
            for k in g.keys() {
                if !keys.contains(k) && !extra.contains(&k.as_str()) {
                    errs.push(format!("{path}: unexpected key '{k}' in {got}"));
                }
            }
        }
        Value::Array(w) => match got.as_array() {
            Some(g) if g.len() == w.len() => {
                for (i, (wv, gv)) in w.iter().zip(g).enumerate() {
                    conforms(wv, gv, subs, extra, &format!("{path}[{i}]"), errs);
                }
            }
            _ => errs.push(format!("{path}: expected {want}, got {got}")),
        },
        Value::String(s) => check_string(s, got, subs, path, errs),
        _ => {
            if want != got {
                errs.push(format!("{path}: expected {want}, got {got}"));
            }
        }
    }
}

#[track_caller]
fn assert_conforms(want: &Value, got: &Value, subs: &Subs, extra: &[&str]) {
    let mut errs = Vec::new();
    conforms(want, got, subs, extra, "$", &mut errs);
    assert!(
        errs.is_empty(),
        "does not conform to the script:\n  {}\nwhole value: {got}",
        errs.join("\n  ")
    );
}

/// An HTTP answer against a `demonio` step: `estado`, `json`, and
/// `www_authenticate` when the step asks for it.
#[track_caller]
fn assert_http(want: &Value, r: &Reply, subs: &Subs) {
    assert_eq!(
        u64::from(r.status),
        want["estado"].as_u64().unwrap(),
        "{}",
        r.body
    );
    let body: Value = serde_json::from_str(&r.body)
        .unwrap_or_else(|_| panic!("the body is not JSON: {:?}", r.body));
    assert_conforms(&want["json"], &body, subs, &[]);
    if want["www_authenticate"] == true {
        assert!(
            r.header("www-authenticate").is_some(),
            "the step requires WWW-Authenticate: {}",
            r.head
        );
    }
}

/// A `demonio.cierre` step against the close the socket ended with.
#[track_caller]
fn assert_close(want: &Value, got: &(u16, String)) {
    assert_eq!(u64::from(got.0), want["codigo"].as_u64().unwrap());
    if let Some(reason) = want.get("motivo") {
        assert_eq!(got.1, reason.as_str().unwrap());
    }
}

/// A client frame from the script, with its example values replaced.
fn frame(template: &Value, replace: &[(&str, Value)]) -> Value {
    match template {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), frame(v, replace)))
                .collect(),
        ),
        Value::String(s) => replace
            .iter()
            .find(|(k, _)| k == s)
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| template.clone()),
        other => other.clone(),
    }
}

/// A `cliente` HTTP/WebSocket step's headers, with placeholders replaced.
fn headers(step: &Value, replace: &[(&str, &str)]) -> Vec<(String, String)> {
    step["cabeceras"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    let mut v = v.as_str().unwrap().to_string();
                    for (from, to) in replace {
                        v = v.replace(from, to);
                    }
                    (k.clone(), v)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn as_refs(h: &[(String, String)]) -> Vec<(&str, &str)> {
    h.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

/// A `cliente.subprotocolos` list, joined for `Sec-WebSocket-Protocol`.
fn protocols(step: &Value, ticket: &str) -> String {
    step["subprotocolos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().replace(EXAMPLE_TICKET, ticket))
        .collect::<Vec<_>>()
        .join(", ")
}

fn ticket_of(r: &Reply) -> String {
    assert_eq!(r.status, 200, "ticket mint refused: {}", r.body);
    r.json()["ticket"].as_str().expect("ticket").to_string()
}

/// Open a socket the browser's way: mint from `ORIGIN_A`, present the ticket.
async fn connect_ticket(d: &Daemon, token: &str) -> (Ws, Value) {
    let t = ticket_of(&mint_ticket(d, token, Some(ORIGIN_A)).await);
    let r = upgrade(
        d,
        &format!("mae.v1, mae.ticket.{t}"),
        &[("Origin", ORIGIN_A)],
    )
    .await;
    assert_eq!(r.status, 101, "{}", r.body);
    let mut ws = r.ws.unwrap();
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    (ws, hello)
}

/// Send a request whose id is the script's (a string) and return its answer.
async fn call_str(ws: &mut Ws, req: Value) -> Value {
    let id = req["id"].clone();
    ws.send(req).await;
    ws.until(|v| v["id"] == id && v.get("method").is_none())
        .await
}

fn ok(resp: &Value) -> bool {
    resp["error"].is_null()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn random_b64url_43() -> String {
    let mut b = [0u8; 32];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut b);
    data_encoding::BASE64URL_NOPAD.encode(&b)
}

// ---------------------------------------------------------------------------
// The scenarios, one runner each
// ---------------------------------------------------------------------------

/// "ticket y conexión": mint, connect, `mae.v1` selected without the ticket,
/// and `session/hello` as the first frame. Also the CORS a browser needs to
/// make the mint call at all.
async fn ticket_and_connect(sc: Value) {
    let d = spawn(Opts {
        max_open_docs: 64,
        ..Opts::default()
    })
    .await;
    let token = d.token(EXAMPLE_PRINCIPAL, 3600);
    let bearer = format!("Bearer {token}");
    let mint_step = step(&sc, 0, "cliente");
    assert_eq!(mint_step["ruta"], "/ws/ticket");
    let replace = [
        ("Bearer <token>", bearer.as_str()),
        ("<origen de la página>", ORIGIN_A),
        ("<el mismo origen>", ORIGIN_A),
    ];
    let h = headers(mint_step, &replace);
    let minted = request(&d, "POST", "/ws/ticket", &as_refs(&h), false).await;
    let subs = Subs::default().with(EXAMPLE_TICKET, Check::Ticket);
    assert_http(step(&sc, 1, "demonio"), &minted, &subs);
    assert_eq!(
        minted.header("access-control-allow-origin").as_deref(),
        Some(ORIGIN_A),
        "a browser withholds the ticket without CORS"
    );
    let preflight = request(
        &d,
        "OPTIONS",
        "/ws/ticket",
        &[
            ("Origin", ORIGIN_A),
            ("Access-Control-Request-Method", "POST"),
            ("Access-Control-Request-Headers", "authorization"),
        ],
        false,
    )
    .await;
    assert_eq!(preflight.status, 204, "{}", preflight.head);
    assert_eq!(
        preflight.header("access-control-allow-origin").as_deref(),
        Some(ORIGIN_A)
    );

    let ticket = ticket_of(&minted);
    let ws_step = step(&sc, 2, "cliente");
    let h = headers(ws_step, &replace);
    let mut r = upgrade(&d, &protocols(ws_step, &ticket), &as_refs(&h)).await;
    assert_eq!(r.status, 101, "{}", r.body);
    let chosen = step(&sc, 3, "demonio")["subprotocolo_elegido"]
        .as_str()
        .unwrap();
    assert_eq!(r.header("sec-websocket-protocol").as_deref(), Some(chosen));
    assert!(!r.head.contains(&ticket), "the ticket is never echoed");

    let ws = r.ws.as_mut().unwrap();
    let first = match ws.next().await {
        Event::Text(v) => v,
        other => panic!("expected session/hello first, got {other:?}"),
    };
    let subs = Subs::default()
        .deadlines()
        .with(EXAMPLE_PRINCIPAL, Check::Eq(json!(EXAMPLE_PRINCIPAL)));
    assert_conforms(&step(&sc, 4, "demonio")["trama"], &first, &subs, &[]);
}

/// "ticket usado, caducado o desconocido": the three refusals conform and are
/// byte for byte the same.
async fn ticket_refusals(sc: Value) {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let origin = [("Origin", ORIGIN_A)];
    let ask = step(&sc, 0, "cliente");
    let offered = |t: &str| {
        ask["subprotocolos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().replace("<usado|caducado|inventado>", t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let used = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
    let first = upgrade(&d, &format!("mae.v1, mae.ticket.{used}"), &origin).await;
    assert_eq!(first.status, 101, "{}", first.body);
    drop(first);
    let expired = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
    tokio::time::sleep(Duration::from_secs(31)).await;

    let want = step(&sc, 1, "demonio");
    let mut seen: Vec<Reply> = Vec::new();
    for t in [used, expired, random_b64url_43()] {
        let r = upgrade(&d, &offered(&t), &origin).await;
        assert_http(want, &r, &Subs::default());
        seen.push(r);
    }
    assert_eq!(
        seen[0].fingerprint(),
        seen[1].fingerprint(),
        "used vs expired"
    );
    assert_eq!(seen[0].body, seen[2].body, "used vs unknown, body");
    assert_eq!(
        seen[0].fingerprint(),
        seen[2].fingerprint(),
        "used vs unknown"
    );
}

/// "credencial en la URL": a live ticket in the query string is refused.
async fn credential_in_url(sc: Value) {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let t = ticket_of(&mint_ticket(&d, &token, None).await);
    let ask = step(&sc, 0, "cliente");
    let path = ask["websocket"]
        .as_str()
        .unwrap()
        .replace(EXAMPLE_TICKET, &t);
    assert!(path.starts_with("/ws?"));
    let h = [("Sec-WebSocket-Protocol".to_string(), protocols(ask, &t))];
    let r = request(&d, "GET", &path, &as_refs(&h), true).await;
    assert_http(step(&sc, 1, "demonio"), &r, &Subs::default());
}

/// "sin versión en común": a valid ticket, no version in common.
async fn no_common_version(sc: Value) {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let t = ticket_of(&mint_ticket(&d, &token, None).await);
    let r = upgrade(&d, &protocols(step(&sc, 0, "cliente"), &t), &[]).await;
    assert_http(step(&sc, 1, "demonio"), &r, &Subs::default());
}

/// "origen fuera de la lista": a mint from an origin not on the list.
async fn origin_not_allowed(sc: Value) {
    let d = spawn(Opts::default()).await;
    let ask = step(&sc, 0, "cliente");
    assert_eq!(ask["ruta"], "/ws/ticket");
    let h = headers(ask, &[]);
    let r = request(&d, "POST", "/ws/ticket", &as_refs(&h), false).await;
    assert_http(step(&sc, 1, "demonio"), &r, &Subs::default());
    assert!(
        r.header("access-control-allow-origin").is_none(),
        "no CORS for an origin off the list"
    );
}

/// "renovar antes del plazo": `session/renew_needed`, then the script's
/// `session/renew` with its string id, answered with the new deadline.
async fn renew_in_time(sc: Value) {
    let d = spawn(Opts {
        renew_lead_secs: 2,
        ..Opts::default()
    })
    .await;
    let (mut ws, hello) = connect_ticket(&d, &d.token("SHA256:alice", 4)).await;
    let first_deadline = deadline_unix(&hello["params"]["auth_deadline"]);
    let needed = ws.until(|v| v["method"] == "session/renew_needed").await;
    let subs = Subs::default().deadlines();
    assert_conforms(&step(&sc, 0, "demonio")["trama"], &needed, &subs, &[]);
    assert!(
        now_unix() < first_deadline,
        "renew_needed precedes the deadline"
    );

    let fresh = d.token("SHA256:alice", 3600);
    let req = frame(
        &step(&sc, 1, "cliente")["trama"],
        &[("<token nuevo>", json!(fresh))],
    );
    let answer = call_str(&mut ws, req).await;
    assert_conforms(&step(&sc, 2, "demonio")["trama"], &answer, &subs, &[]);
    assert!(deadline_unix(&answer["result"]["auth_deadline"]) > first_deadline + 60);
}

/// "renovar con otra identidad": closed `4403 principal_changed`.
async fn renew_other_identity(sc: Value) {
    let d = spawn(Opts::default()).await;
    let (mut ws, _) = connect_ticket(&d, &d.token("SHA256:alice", 3600)).await;
    let bob = d.token("SHA256:bob", 3600);
    let req = frame(
        &step(&sc, 0, "cliente")["trama"],
        &[("<token de otro sujeto>", json!(bob))],
    );
    ws.send(req).await;
    assert_close(&step(&sc, 1, "demonio")["cierre"], &ws.closed().await);
}

/// "plazo vencido": no renewal, closed `4401 token_expired`.
async fn deadline_missed(sc: Value) {
    let d = spawn(Opts {
        renew_lead_secs: 1,
        ..Opts::default()
    })
    .await;
    let (mut ws, hello) = connect_ticket(&d, &d.token("SHA256:alice", 3)).await;
    let deadline = deadline_unix(&hello["params"]["auth_deadline"]);
    let close = ws.closed().await;
    assert!(
        now_unix() >= deadline,
        "closed no earlier than the deadline"
    );
    assert_close(&step(&sc, 0, "demonio")["cierre"], &close);
}

/// A writer on `node` of `KB`, authoring under the principal's current-epoch
/// client id, from the node's state as `kb/join` returned it.
fn node_writer(joined: &Value, node: &str, principal: &str, epoch: u64) -> KbNodeDoc {
    let entry = joined["result"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == node)
        .unwrap_or_else(|| panic!("kb/join returned no node '{node}': {joined}"));
    let state = base64_to_update(entry["state"].as_str().unwrap()).unwrap();
    KbNodeDoc::from_bytes_with_client_id(&state, derive_kb_client_id(principal, epoch)).unwrap()
}

fn epoch_of(joined: &Value) -> u64 {
    joined["result"]["epoch"]
        .as_u64()
        .unwrap_or_else(|| panic!("kb/join reports no epoch: {joined}"))
}

/// Seeded filler, so each flood update replaces its value outright.
fn filler(seed: u64, len: usize) -> String {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b'a' + (x % 26) as u8)
        })
        .collect()
}

/// The flood that makes a document slow. The writer applies `FLOOD_TOTAL`
/// updates to n1 as fast as the daemon takes them; the reader reads only
/// `SLOW_BATCH` frames per `SLOW_TICK`. Its socket therefore drains, but far
/// slower than the updates arrive, so the backlog fills the kernel's buffers,
/// the pipe and outbound channel between the session and the socket, and then
/// the session's event queue, whose overflow closes the document (B4).
///
/// Reading a little on every tick, rather than not at all, is what makes this
/// independent of the machine's speed: the daemon's send to that client never
/// waits long, so the 5 s write timeout that drops a client that does not read
/// (D4) is never in play, and the document closes after however long the
/// flood takes. Measured on an idle debug build: it closes after ~700 updates.
const FLOOD_BYTES: usize = 4 * 1024;
const FLOOD_TOTAL: u64 = 1500;
const FLOOD_WINDOW: u64 = 50;
/// ~5.6 KB a frame: 40 frames reopen a loopback receive window (64 KB MSS)
/// by well over two segments, so each tick lets the daemon's send progress.
const SLOW_BATCH: usize = 40;
const SLOW_TICK: Duration = Duration::from_secs(1);

/// Apply `FLOOD_TOTAL` n1 updates in windows, then raise `done`.
async fn flood(w: &mut Ws, doc: &mut KbNodeDoc, done: &std::sync::atomic::AtomicBool) {
    let mut id = 100u64;
    while id < 100 + FLOOD_TOTAL {
        for _ in 0..FLOOD_WINDOW {
            let u = update_to_base64(&doc.set_meta("pad", &filler(id, FLOOD_BYTES)));
            w.send(
                json!({"jsonrpc": "2.0", "id": id, "method": "kb/node_update",
                "params": {"kb_id": KB, "node_id": "n1", "update": u}}),
            )
            .await;
            id += 1;
        }
        let last = id - 1;
        let resp = w
            .until(|v| v["id"] == last && v.get("method").is_none())
            .await;
        assert!(ok(&resp), "{resp}");
    }
    done.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Read slowly until the flood is done, then at full speed, until the
/// document-closed notice; it may arrive in either phase.
async fn read_slowly_until_closed(r: &mut Ws, done: &std::sync::atomic::AtomicBool) -> Value {
    let closed = |v: &Value| v["method"] == "session/doc_closed";
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        for _ in 0..SLOW_BATCH {
            match r.next().await {
                Event::Text(v) if closed(&v) => return v,
                Event::Text(_) => {}
                other => panic!("the slow session ended: {other:?}"),
            }
        }
        tokio::time::sleep(SLOW_TICK).await;
    }
    r.until(closed).await
}

/// "dos documentos en un socket; uno lento": the script's join and updates,
/// then a document that cannot keep up is closed for its session alone,
/// `slow_consumer`, while the session's other document keeps flowing; the
/// closed one re-opens and resyncs.
async fn slow_document(sc: Value) {
    let d = spawn(Opts::default()).await;
    let me = "SHA256:writer";
    let token = d.token(me, 3600);
    let (mut r, hello) = connect_ticket(&d, &token).await;
    let share = kb_share(KB, &[("n1", "uno", "a"), ("n2", "dos", "b")]);
    assert!(ok(&r.call(1, "kb/share", share).await));

    let joined = call_str(&mut r, step(&sc, 0, "cliente")["trama"].clone()).await;
    assert!(ok(&joined), "{joined}");
    assert_eq!(joined["id"], "j-1");
    // The script's identity rule: principal as in session/hello, epoch from kb/join.
    assert_eq!(joined["result"]["principal"], hello["params"]["principal"]);
    let epoch = epoch_of(&joined);
    let mut n1 = node_writer(&joined, "n1", me, epoch);
    let mut n2 = node_writer(&joined, "n2", me, epoch);
    for (i, (node, doc)) in [("n1", &mut n1), ("n2", &mut n2)].into_iter().enumerate() {
        let update = update_to_base64(&doc.set_title(&format!("{node} editado")));
        let req = frame(
            &step(&sc, 1 + i, "cliente")["trama"],
            &[("<base64>", json!(update))],
        );
        assert_eq!(req["params"]["node_id"], node);
        let answer = call_str(&mut r, req).await;
        assert!(ok(&answer), "{answer}");
    }

    // A second session of the same principal floods n1 while `r` does not read.
    let (mut w, _) = connect_ticket(&d, &token).await;
    let wj = w.call(1, "kb/join", json!({"kb_id": KB})).await;
    let mut wn1 = node_writer(&wj, "n1", me, epoch);
    let mut wn2 = node_writer(&wj, "n2", me, epoch);
    let done = std::sync::atomic::AtomicBool::new(false);
    let want = &step(&sc, 3, "demonio")["trama"];
    let (notice, ()) = tokio::join!(
        read_slowly_until_closed(&mut r, &done),
        flood(&mut w, &mut wn1, &done)
    );
    let subs = Subs::default().with(
        "<nombre del documento en el sobre>",
        Check::Eq(json!(format!("kbn:{KB}:n1"))),
    );
    assert_conforms(want, &notice, &subs, &[]);

    // "n2 sigue": the same session still receives n2.
    let u = update_to_base64(&wn2.set_title("dos sigue"));
    let resp = w
        .call(
            9000,
            "kb/node_update",
            json!({"kb_id": KB, "node_id": "n2", "update": u}),
        )
        .await;
    assert!(ok(&resp), "{resp}");
    let n2_doc = format!("kbn:{KB}:n2");
    let got = r
        .until(|v| {
            v["method"] == "notifications/sync_update"
                && v["params"]["event"]["data"]["buffer_name"] == n2_doc
                && v["params"]["event"]["data"]["update_base64"] == u
        })
        .await;
    assert!(got.is_object());

    // "vuelve a abrir n1 (se resincroniza)": the reopened copy has every edit.
    let fetched = r
        .call(9001, "kb/node_fetch", json!({"kb_id": KB, "node_id": "n1"}))
        .await;
    assert!(ok(&fetched), "{fetched}");
    let state = base64_to_update(fetched["result"]["state"].as_str().unwrap()).unwrap();
    let resynced = KbNodeDoc::from_bytes(&state).unwrap();
    assert_eq!(
        resynced.get_meta("pad"),
        wn1.get_meta("pad"),
        "the resynced n1 holds the last flood edit"
    );
    assert_eq!(resynced.title(), "n1 editado");
}

/// "miembro quitado entre renovaciones": at the next renewal Bob loses that
/// KB's documents, one notice each, and keeps the session and his own KB.
async fn member_removed(sc: Value) {
    let d = spawn(Opts {
        renew_lead_secs: 2,
        ..Opts::default()
    })
    .await;
    let (mut alice, _) = connect_ticket(&d, &d.token("SHA256:alice", 3600)).await;
    assert!(ok(&alice
        .call(1, "kb/share", kb_share(KB, &[("n1", "uno", "a")]))
        .await));
    let bob_in = json!({"kb_id": KB, "member": "SHA256:bob", "role": "viewer", "label": "bob"});
    assert!(ok(&alice.call(2, "kb/add_member", bob_in).await));

    let (mut bob, _) = connect_ticket(&d, &d.token("SHA256:bob", 4)).await;
    assert!(ok(&bob.call(1, "kb/share", kb_share("kb-b", &[])).await));
    assert!(ok(&bob.call(2, "kb/join", json!({"kb_id": KB})).await));
    let bob_out = json!({"kb_id": KB, "member": "SHA256:bob"});
    assert!(ok(&alice.call(3, "kb/remove_member", bob_out).await));

    bob.until(|v| v["method"] == "session/renew_needed").await;
    let renew = json!({"token": d.token("SHA256:bob", 3600)});
    assert!(ok(&bob.call(3, "session/renew", renew).await));

    let want = &step(&sc, 0, "demonio")["trama"];
    // The collection document's notice is the node one's without `node_id`.
    let mut want_coll = want.clone();
    want_coll["params"]
        .as_object_mut()
        .unwrap()
        .remove("node_id");
    let mut closed = std::collections::BTreeSet::new();
    while closed.len() < 2 {
        let v = bob.until(|v| v["method"] == "session/doc_closed").await;
        let doc = v["params"]["doc"].as_str().unwrap_or("").to_string();
        let template = if doc.starts_with("kbn:") {
            want
        } else {
            &want_coll
        };
        let subs =
            Subs::default().with("<nombre del documento en el sobre>", Check::Eq(json!(doc)));
        assert_conforms(template, &v, &subs, &[]);
        closed.insert(doc);
    }
    let expect: std::collections::BTreeSet<String> =
        [format!("kbc:{KB}"), format!("kbn:{KB}:n1")].into();
    assert_eq!(closed, expect);
    let own = bob
        .call(4, "sync/state_vector", json!({"doc": "kbc:kb-b"}))
        .await;
    assert!(ok(&own), "his other KB and the session stay open: {own}");
}

/// "trama demasiado grande": a frame of exactly `limits.max_frame_bytes` is
/// served; one byte more closes the socket `1009`.
///
/// The client's send buffer is small, so it is still writing the oversized
/// frame when the daemon refuses it: a daemon that then drops the connection
/// with that frame unread makes the kernel reset it, and the client loses the
/// `1009` it was sent (a browser reports 1006). Without the small buffer the
/// whole frame fits in the kernel's buffers and that defect hides.
async fn frame_too_large(sc: Value) {
    let d = spawn(Opts::default()).await;
    let token = d.token("SHA256:alice", 3600);
    let t = ticket_of(&mint_ticket(&d, &token, Some(ORIGIN_A)).await);
    let protocols = format!("mae.v1, mae.ticket.{t}");
    let r = upgrade_small_send_buffer(&d, &protocols, &[("Origin", ORIGIN_A)], 32 * 1024).await;
    assert_eq!(r.status, 101, "{}", r.body);
    let mut ws = r.ws.unwrap();
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    let cap = hello["params"]["limits"]["max_frame_bytes"]
        .as_u64()
        .unwrap() as usize;
    let sized = |n: usize, id: u64| {
        let head = format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"$/ping\",\"pad\":\"");
        let pad = "x".repeat(n - head.len() - 2);
        let s = format!("{head}{pad}\"}}");
        assert_eq!(s.len(), n);
        s
    };
    ws.send_raw(sized(cap, 1)).await;
    let at_cap = ws.until(|v| v["id"] == 1).await;
    assert!(
        ok(&at_cap),
        "a frame of exactly the cap is served: {at_cap}"
    );
    ws.send_raw(sized(cap + 1, 2)).await;
    assert_close(&step(&sc, 1, "demonio")["cierre"], &ws.closed().await);
}

fn awareness_update(client_id: u64, state: &str) -> String {
    use mae_sync::yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
    use mae_sync::yrs::updates::encoder::Encode;
    let mut u = AwarenessUpdate {
        clients: Default::default(),
    };
    u.clients.insert(
        mae_sync::yrs::block::ClientID::new(client_id),
        AwarenessUpdateEntry {
            clock: 1,
            json: state.into(),
        },
    );
    update_to_base64(&u.encode_v1())
}

/// "presencia": the script's `sync/awareness` notification, relayed to another
/// member as `sync/awareness` with the sender's stamped principal per client
/// id, whatever `user` the sender claimed.
async fn presence(sc: Value) {
    let d = spawn(Opts::default()).await;
    let (mut alice, _) = connect_ticket(&d, &d.token("SHA256:alice", 3600)).await;
    assert!(ok(&alice
        .call(1, "kb/share", kb_share(KB, &[("n1", "uno", "a")]))
        .await));
    let bob_in = json!({"kb_id": KB, "member": "SHA256:bob", "role": "viewer", "label": "bob"});
    assert!(ok(&alice.call(2, "kb/add_member", bob_in).await));
    let (mut bob, _) = connect_ticket(&d, &d.token("SHA256:bob", 3600)).await;
    assert!(ok(&bob.call(1, "kb/join", json!({"kb_id": KB})).await));

    let send = &step(&sc, 0, "cliente")["trama"];
    assert!(
        send.get("id").is_none(),
        "the script sends presence as a notification"
    );
    let bob_says = awareness_update(2002, r#"{"user":{"name":"bob"}}"#);
    bob.send(frame(
        send,
        &[(send["params"]["update"].as_str().unwrap(), json!(bob_says))],
    ))
    .await;
    // A round trip on the same session: bob's presence has been processed.
    assert!(ok(&bob.call(2, "kb/join", json!({"kb_id": KB})).await));

    let lie = awareness_update(1001, r#"{"user":{"name":"SHA256:bob"}}"#);
    alice
        .send(frame(
            send,
            &[(send["params"]["update"].as_str().unwrap(), json!(lie))],
        ))
        .await;
    let got = bob.until(|v| v["method"] == "sync/awareness").await;
    let subs = Subs::default()
        .with("<base64>", Check::Eq(json!(lie)))
        .with("<clientID>", Check::Eq(json!("1001")))
        .with("<principal sellado>", Check::Eq(json!("SHA256:alice")));
    assert_conforms(&step(&sc, 1, "demonio")["trama"], &got, &subs, &["doc"]);
}

/// "emisor externo en el primer corte": a token from another issuer is
/// refused at the mint.
async fn external_issuer(sc: Value) {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let d = spawn(Opts::default()).await;
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("idp-signing-key".to_string());
    let claims = json!({"sub": "alice@example.com", "aud": AUDIENCE,
        "iss": "https://idp.example.com", "exp": 4_000_000_000u64});
    let external = encode(&header, &claims, &EncodingKey::from_secret(b"k")).unwrap();
    let bearer = format!("Bearer {external}");
    let ask = step(&sc, 0, "cliente");
    let h = headers(
        ask,
        &[("Bearer <token de un emisor externo>", bearer.as_str())],
    );
    let r = request(&d, "POST", "/ws/ticket", &as_refs(&h), false).await;
    assert_http(step(&sc, 1, "demonio"), &r, &Subs::default());
}

type Runner = fn(Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;

macro_rules! runner {
    ($f:ident) => {
        (|sc| Box::pin($f(sc))) as Runner
    };
}

/// Every scenario of the script, replayed against its own daemon. Scenarios
/// run concurrently (the slowest waits out a 30-second ticket), and every
/// failure is reported, not only the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemon_plays_the_client_script() {
    let script = script();
    let runners: Vec<(&str, Runner)> = vec![
        ("ticket y conexión", runner!(ticket_and_connect)),
        (
            "ticket usado, caducado o desconocido",
            runner!(ticket_refusals),
        ),
        ("credencial en la URL", runner!(credential_in_url)),
        ("sin versión en común", runner!(no_common_version)),
        ("origen fuera de la lista", runner!(origin_not_allowed)),
        ("renovar antes del plazo", runner!(renew_in_time)),
        ("renovar con otra identidad", runner!(renew_other_identity)),
        ("plazo vencido", runner!(deadline_missed)),
        (
            "dos documentos en un socket; uno lento",
            runner!(slow_document),
        ),
        (
            "miembro quitado entre renovaciones",
            runner!(member_removed),
        ),
        ("trama demasiado grande", runner!(frame_too_large)),
        ("presencia", runner!(presence)),
        (
            "emisor externo en el primer corte",
            runner!(external_issuer),
        ),
    ];
    // Every scenario in the script has a runner, and nothing runs unscripted.
    let names: Vec<&str> = script["escenarios"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["nombre"].as_str().unwrap())
        .collect();
    let ours: Vec<&str> = runners.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names, ours,
        "the script's scenarios and this test's runners"
    );

    // The slow-document scenario is CPU-bound (a flood through a debug
    // build), so it runs alone, after the rest, not competing with them.
    const ALONE: &str = "dos documentos en un socket; uno lento";
    let (alone, together): (Vec<_>, Vec<_>) = runners.into_iter().partition(|(n, _)| *n == ALONE);
    let mut set = tokio::task::JoinSet::new();
    for (name, run) in together {
        let sc = scenario(&script, name).clone();
        set.spawn(async move { (name, tokio::spawn(run(sc)).await) });
    }
    let mut results = Vec::new();
    while let Some(done) = set.join_next().await {
        results.push(done.expect("scenario task"));
    }
    for (name, run) in alone {
        let sc = scenario(&script, name).clone();
        results.push((name, tokio::spawn(run(sc)).await));
    }
    let mut failed = Vec::new();
    for (name, result) in results {
        if let Err(e) = result {
            let why = e
                .try_into_panic()
                .ok()
                .and_then(|p| {
                    p.downcast_ref::<String>()
                        .cloned()
                        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                })
                .unwrap_or_else(|| "cancelled".to_string());
            failed.push(format!("[{name}] {why}"));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of 13 scenarios differ from the script:\n\n{}",
        failed.len(),
        failed.join("\n\n")
    );
}
