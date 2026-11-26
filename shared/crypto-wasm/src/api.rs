//! The facade's operations: a JSON request in, a JSON response out. Safe
//! Rust only; `lib.rs` moves the bytes across the ABI. The contract (every
//! operation's request and result, the error codes, the backup format) is
//! written out in docs/E2E_ENCRYPTION.md, "Browser key material"; this file
//! and `js/mae-crypto.d.ts` must say the same.
//!
//! Every response is `{"ok":true,"result":R}` or
//! `{"ok":false,"code":"<code>","error":"<message>"}`. A request is a JSON
//! object with exactly the keys its operation names (an unknown or missing
//! key is `bad-request`, never a default). Bytes travel as standard base64
//! with padding. A member key is named by the handle (`key`, a number) the
//! module gave it; its seed stays inside the module unless `export_seed` or
//! `backup_seal` is called for it.
//!
//! Results are serialised from structs, so their key order is fixed by this
//! file, not by serde_json's map type (which a cargo feature elsewhere in
//! the workspace can change).

use std::borrow::Cow;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mae_crypto::backup::{self, BackupError, KdfParams};
use mae_crypto::content_crypto::{self as cc, ContentKey, CryptoError};
use mae_crypto::content_op::{ContentOp, HeaderError};
use mae_crypto::identity_challenge::{self as ic, Challenge, ChallengeRefusal, Claimant};
use mae_crypto::member::{self, MemberKey};
use mae_crypto::rng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::keystore::{self, StoreError};

/// The contract version (`mae-crypto-vX.Y.Z` tags), reported by `version`.
pub const API_VERSION: &str = env!("CARGO_PKG_VERSION");

/// An operation, by its ABI number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Version = 1,
    Generate = 2,
    ImportSeed = 3,
    ExportSeed = 4,
    Forget = 5,
    KeyInfo = 6,
    Fingerprint = 7,
    CheckChallenge = 8,
    SignChallenge = 9,
    BackupSeal = 10,
    BackupOpen = 11,
    BackupParams = 12,
    ContentKeyGenerate = 13,
    ContentSeal = 14,
    ContentOpen = 15,
    ContentWrap = 16,
    ContentUnwrap = 17,
    SignOp = 18,
    OpCanonicalBytes = 19,
    VerifyOp = 20,
}

impl Op {
    /// Every operation, in ABI order.
    pub const ALL: [Op; 20] = [
        Op::Version,
        Op::Generate,
        Op::ImportSeed,
        Op::ExportSeed,
        Op::Forget,
        Op::KeyInfo,
        Op::Fingerprint,
        Op::CheckChallenge,
        Op::SignChallenge,
        Op::BackupSeal,
        Op::BackupOpen,
        Op::BackupParams,
        Op::ContentKeyGenerate,
        Op::ContentSeal,
        Op::ContentOpen,
        Op::ContentWrap,
        Op::ContentUnwrap,
        Op::SignOp,
        Op::OpCanonicalBytes,
        Op::VerifyOp,
    ];

    pub fn from_u32(n: u32) -> Option<Op> {
        Op::ALL.into_iter().find(|o| *o as u32 == n)
    }
}

/// An error response.
#[derive(Debug)]
pub struct ApiError {
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    fn new(code: &'static str, message: impl std::fmt::Display) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl From<ChallengeRefusal> for ApiError {
    fn from(r: ChallengeRefusal) -> Self {
        Self::new(r.code(), &r)
    }
}

impl From<BackupError> for ApiError {
    fn from(e: BackupError) -> Self {
        Self::new(e.code(), &e)
    }
}

impl From<CryptoError> for ApiError {
    fn from(e: CryptoError) -> Self {
        match e {
            CryptoError::Decrypt => Self::new("decrypt", "wrong key, or the data was altered"),
            CryptoError::Malformed => Self::new("malformed", "malformed or hostile input"),
            CryptoError::BadKey => Self::new("bad-key", "not a valid public key"),
        }
    }
}

impl From<HeaderError> for ApiError {
    fn from(e: HeaderError) -> Self {
        match e {
            HeaderError::EmptyField(_) => Self::new("empty-field", e),
            HeaderError::NulInField(_) => Self::new("nul-in-field", e),
        }
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Full => Self::new(
                "keystore-full",
                format!("{} keys are held; forget one first", keystore::MAX_KEYS),
            ),
            StoreError::Exhausted => Self::new("keystore-full", "no handles left in this instance"),
        }
    }
}

fn no_entropy() -> ApiError {
    ApiError::new("no-entropy", rng::RandomUnavailable)
}

/// Runs operation `op` on `request` (JSON bytes). Total: any input gives a
/// JSON response.
pub fn call(op: u32, request: &[u8]) -> String {
    let result = match Op::from_u32(op) {
        Some(op) => run(op, request),
        None => Err(ApiError::new(
            "unknown-op",
            format!("unknown operation {op}"),
        )),
    };
    match result {
        Ok(r) => format!("{{\"ok\":true,\"result\":{r}}}"),
        Err(e) => format!(
            "{{\"ok\":false,\"code\":{},\"error\":{}}}",
            json_str(e.code),
            json_str(&e.message)
        ),
    }
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("a string serialises")
}

fn run(op: Op, r: &[u8]) -> Result<String, ApiError> {
    match op {
        Op::Version => version(r),
        Op::Generate => generate(r),
        Op::ImportSeed => import_seed(r),
        Op::ExportSeed => export_seed(r),
        Op::Forget => forget(r),
        Op::KeyInfo => key_info(r),
        Op::Fingerprint => fingerprint(r),
        Op::CheckChallenge => check_challenge(r),
        Op::SignChallenge => sign_challenge(r),
        Op::BackupSeal => backup_seal(r),
        Op::BackupOpen => backup_open(r),
        Op::BackupParams => backup_params(r),
        Op::ContentKeyGenerate => content_key_generate(r),
        Op::ContentSeal => content_seal(r),
        Op::ContentOpen => content_open(r),
        Op::ContentWrap => content_wrap(r),
        Op::ContentUnwrap => content_unwrap(r),
        Op::SignOp => sign_op(r),
        Op::OpCanonicalBytes => op_canonical_bytes(r),
        Op::VerifyOp => verify_op(r),
    }
}

// ---- requests ----

/// Parses a request. The error names the kind of problem and where, never
/// serde's message: that quotes the input (a seed, a passphrase, a key).
fn parse<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, ApiError> {
    serde_json::from_slice(bytes).map_err(|e| {
        use serde_json::error::Category;
        let what = match e.classify() {
            Category::Data => "a key is missing, unknown, or of the wrong type",
            Category::Syntax | Category::Eof | Category::Io => "not valid JSON",
        };
        ApiError::new(
            "bad-request",
            format!(
                "the request is not this operation's JSON object: {what} (line {}, column {})",
                e.line(),
                e.column()
            ),
        )
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyReq {
    key: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedReq<'a> {
    #[serde(borrow)]
    seed: Cow<'a, str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicReq<'a> {
    #[serde(borrow)]
    public_key: Cow<'a, str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeReq<'a> {
    key: u32,
    #[serde(borrow)]
    issuer: Cow<'a, str>,
    #[serde(borrow)]
    subject: Cow<'a, str>,
    #[serde(borrow)]
    audience: Cow<'a, str>,
    #[serde(borrow)]
    challenge: Cow<'a, str>,
    now: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KdfReq {
    m: u32,
    t: u32,
    p: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SealReq<'a> {
    key: u32,
    #[serde(borrow)]
    passphrase: Cow<'a, str>,
    #[serde(default)]
    kdf: Option<KdfReq>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenReq<'a> {
    #[serde(borrow)]
    envelope: Cow<'a, str>,
    #[serde(borrow)]
    passphrase: Cow<'a, str>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvelopeReq<'a> {
    #[serde(borrow)]
    envelope: Cow<'a, str>,
}

/// A content operation's request: the content key and the one operand the
/// operation names (`plaintext`, `sealed` or `wrap_public`).
macro_rules! content_req {
    ($name:ident, $field:ident) => {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $name<'a> {
            #[serde(borrow)]
            content_key: Cow<'a, str>,
            #[serde(borrow)]
            $field: Cow<'a, str>,
        }
    };
}
content_req!(SealContentReq, plaintext);
content_req!(OpenContentReq, sealed);
content_req!(WrapContentReq, wrap_public);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnwrapReq<'a> {
    key: u32,
    #[serde(borrow)]
    wrapped: Cow<'a, str>,
}

fn bytes(field: &str, b64: &str) -> Result<Zeroizing<Vec<u8>>, ApiError> {
    STANDARD.decode(b64).map(Zeroizing::new).map_err(|_| {
        ApiError::new(
            "bad-request",
            format!("{field} is not canonical padded base64"),
        )
    })
}

fn bytes32(field: &str, b64: &str) -> Result<Zeroizing<[u8; 32]>, ApiError> {
    let v = bytes(field, b64)?;
    let mut out = Zeroizing::new([0u8; 32]);
    if v.len() != out.len() {
        let n = v.len();
        return Err(ApiError::new(
            "bad-request",
            format!("{field} is {n} bytes, not 32"),
        ));
    }
    out.copy_from_slice(&v);
    Ok(out)
}

/// Runs `f` on the held key `handle`, or `no-key`.
fn held<R>(handle: u32, f: impl FnOnce(&MemberKey) -> R) -> Result<R, ApiError> {
    keystore::with(handle, f).ok_or_else(|| {
        ApiError::new(
            "no-key",
            format!("no key {handle} in this instance (forgotten, or never given)"),
        )
    })
}

// ---- results ----

fn out<T: Serialize>(v: &T) -> Result<String, ApiError> {
    Ok(serde_json::to_string(v).expect("a result serialises"))
}

#[derive(Serialize)]
struct KeyInfo {
    key: u32,
    public_key: String,
    fingerprint: String,
    wrap_public: String,
}

fn info(handle: u32) -> Result<String, ApiError> {
    let i = held(handle, |k| KeyInfo {
        key: handle,
        public_key: k.public_key_b64(),
        fingerprint: k.fingerprint(),
        wrap_public: STANDARD.encode(k.wrap_public()),
    })?;
    out(&i)
}

#[derive(Serialize)]
struct ChallengeOut<'a> {
    domain: &'a str,
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    fingerprint: &'a str,
    nonce: &'a str,
    expires_at: u64,
}

impl<'a> From<&'a Challenge> for ChallengeOut<'a> {
    fn from(c: &'a Challenge) -> Self {
        Self {
            domain: &c.domain,
            iss: &c.iss,
            sub: &c.sub,
            aud: &c.aud,
            fingerprint: &c.fingerprint,
            nonce: &c.nonce,
            expires_at: c.expires_at,
        }
    }
}

// ---- operations ----

fn version(r: &[u8]) -> Result<String, ApiError> {
    parse::<Empty>(r)?;
    #[derive(Serialize)]
    struct V {
        version: &'static str,
        backup_version: u64,
        challenge_domain: &'static str,
        content_op_domain: &'static str,
    }
    out(&V {
        version: API_VERSION,
        backup_version: backup::VERSION,
        challenge_domain: ic::DOMAIN_TAG,
        content_op_domain: mae_crypto::content_op::CONTENT_OP_DOMAIN,
    })
}

fn generate(r: &[u8]) -> Result<String, ApiError> {
    parse::<Empty>(r)?;
    let key = MemberKey::generate().map_err(|_| no_entropy())?;
    info(keystore::insert(key)?)
}

fn import_seed(r: &[u8]) -> Result<String, ApiError> {
    let req: SeedReq = parse(r)?;
    let seed = bytes32("seed", &req.seed)?;
    info(keystore::insert(MemberKey::from_seed(&seed))?)
}

fn export_seed(r: &[u8]) -> Result<String, ApiError> {
    let req: KeyReq = parse(r)?;
    let seed = Zeroizing::new(held(req.key, |k| STANDARD.encode(*k.seed()))?);
    #[derive(Serialize)]
    struct S<'a> {
        seed: &'a str,
    }
    out(&S { seed: &seed })
}

fn forget(r: &[u8]) -> Result<String, ApiError> {
    let req: KeyReq = parse(r)?;
    #[derive(Serialize)]
    struct F {
        forgotten: bool,
    }
    out(&F {
        forgotten: keystore::forget(req.key),
    })
}

fn key_info(r: &[u8]) -> Result<String, ApiError> {
    let req: KeyReq = parse(r)?;
    info(req.key)
}

fn fingerprint(r: &[u8]) -> Result<String, ApiError> {
    let req: PublicReq = parse(r)?;
    let public = member::decode_public_key(&req.public_key).ok_or_else(|| {
        ApiError::new(
            "bad-public-key",
            "public_key is not 32 bytes of padded base64 naming an Ed25519 point",
        )
    })?;
    #[derive(Serialize)]
    struct F {
        fingerprint: String,
    }
    out(&F {
        fingerprint: member::fingerprint(&public),
    })
}

fn claimant<'a>(req: &'a ChallengeReq<'_>) -> Claimant<'a> {
    Claimant {
        issuer: &req.issuer,
        subject: &req.subject,
        audience: &req.audience,
    }
}

fn check_challenge(r: &[u8]) -> Result<String, ApiError> {
    let req: ChallengeReq = parse(r)?;
    let c = held(req.key, |k| {
        ic::check_challenge(&k.fingerprint(), claimant(&req), &req.challenge, req.now)
    })??;
    #[derive(Serialize)]
    struct C<'a> {
        challenge: ChallengeOut<'a>,
    }
    out(&C {
        challenge: (&c).into(),
    })
}

fn sign_challenge(r: &[u8]) -> Result<String, ApiError> {
    let req: ChallengeReq = parse(r)?;
    let (c, body) = held(req.key, |k| {
        ic::sign_challenge(k, claimant(&req), &req.challenge, req.now)
    })??;
    #[derive(Serialize)]
    struct Body<'a> {
        challenge_id: &'a str,
        public_key: &'a str,
        signature: &'a str,
    }
    #[derive(Serialize)]
    struct S<'a> {
        challenge: ChallengeOut<'a>,
        body: Body<'a>,
    }
    let field = |k: &str| body[k].as_str().unwrap_or_default().to_string();
    let (id, public, sig) = (
        field("challenge_id"),
        field("public_key"),
        field("signature"),
    );
    out(&S {
        challenge: (&c).into(),
        body: Body {
            challenge_id: &id,
            public_key: &public,
            signature: &sig,
        },
    })
}

fn backup_seal(r: &[u8]) -> Result<String, ApiError> {
    let req: SealReq = parse(r)?;
    let params = match &req.kdf {
        Some(k) => KdfParams {
            m: k.m,
            t: k.t,
            p: k.p,
        },
        None => KdfParams::DEFAULT,
    };
    let envelope = held(req.key, |k| {
        backup::seal(&k.seed(), &req.passphrase, params)
    })??;
    #[derive(Serialize)]
    struct E {
        envelope: String,
    }
    out(&E { envelope })
}

fn backup_open(r: &[u8]) -> Result<String, ApiError> {
    let req: OpenReq = parse(r)?;
    let seed = backup::open(&req.envelope, &req.passphrase)?;
    info(keystore::insert(MemberKey::from_seed(&seed))?)
}

fn backup_params(r: &[u8]) -> Result<String, ApiError> {
    let req: EnvelopeReq = parse(r)?;
    let h = backup::header_of(&req.envelope)?;
    #[derive(Serialize)]
    struct P<'a> {
        fingerprint: &'a str,
        m: u32,
        t: u32,
        p: u32,
    }
    out(&P {
        fingerprint: &h.fingerprint,
        m: h.params.m,
        t: h.params.t,
        p: h.params.p,
    })
}

#[derive(Serialize)]
struct ContentKeyOut<'a> {
    content_key: &'a str,
}

fn content_key_generate(r: &[u8]) -> Result<String, ApiError> {
    parse::<Empty>(r)?;
    let mut k = Zeroizing::new([0u8; 32]);
    rng::fill(&mut k[..]).map_err(|_| no_entropy())?;
    let b64 = Zeroizing::new(STANDARD.encode(*k));
    out(&ContentKeyOut { content_key: &b64 })
}

fn content_key(b64: &str) -> Result<ContentKey, ApiError> {
    Ok(ContentKey::from_bytes(*bytes32("content_key", b64)?))
}

fn content_seal(r: &[u8]) -> Result<String, ApiError> {
    let req: SealContentReq = parse(r)?;
    let plain = bytes("plaintext", &req.plaintext)?;
    let sealed = cc::encrypt(&content_key(&req.content_key)?, &plain);
    #[derive(Serialize)]
    struct S {
        sealed: String,
    }
    out(&S {
        sealed: STANDARD.encode(sealed),
    })
}

fn content_open(r: &[u8]) -> Result<String, ApiError> {
    let req: OpenContentReq = parse(r)?;
    let sealed = bytes("sealed", &req.sealed)?;
    let plain = Zeroizing::new(cc::decrypt(&content_key(&req.content_key)?, &sealed)?);
    let b64 = Zeroizing::new(STANDARD.encode(&*plain));
    #[derive(Serialize)]
    struct P<'a> {
        plaintext: &'a str,
    }
    out(&P { plaintext: &b64 })
}

fn content_wrap(r: &[u8]) -> Result<String, ApiError> {
    let req: WrapContentReq = parse(r)?;
    let wrap_public = bytes32("wrap_public", &req.wrap_public)?;
    let wrapped = cc::wrap_to_member(&content_key(&req.content_key)?, &wrap_public)?;
    #[derive(Serialize)]
    struct W {
        wrapped: String,
    }
    out(&W {
        wrapped: STANDARD.encode(wrapped),
    })
}

fn content_unwrap(r: &[u8]) -> Result<String, ApiError> {
    let req: UnwrapReq = parse(r)?;
    let wrapped = bytes("wrapped", &req.wrapped)?;
    let ck = held(req.key, |k| k.unwrap_content_key(&wrapped))??;
    let b64 = Zeroizing::new(STANDARD.encode(ck.as_bytes()));
    out(&ContentKeyOut { content_key: &b64 })
}

/// `sign_op`'s request: the op's typed fields. The module builds the signed
/// message itself (ADR-036 D1); the author is the held key's own fingerprint.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignOpReq<'a> {
    key: u32,
    #[serde(borrow)]
    kb_id: Cow<'a, str>,
    #[serde(borrow)]
    node_id: Cow<'a, str>,
    #[serde(borrow)]
    base_sv: Cow<'a, str>,
    /// Decimal text: an epoch is a random u64 (#72), beyond what a JavaScript
    /// number holds exactly.
    #[serde(borrow)]
    epoch: Cow<'a, str>,
    issued_at: u64,
    #[serde(borrow)]
    update: Cow<'a, str>,
}

/// A u64 written as canonical decimal text: digits only, no sign, no leading
/// zero (except "0"), in range.
fn decimal_u64(field: &str, s: &str) -> Result<u64, ApiError> {
    let canonical =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    canonical
        .then(|| s.parse::<u64>().ok())
        .flatten()
        .ok_or_else(|| ApiError::new("bad-request", format!("{field} is not a decimal u64")))
}

fn sign_op(r: &[u8]) -> Result<String, ApiError> {
    let req: SignOpReq = parse(r)?;
    let base_sv = bytes("base_sv", &req.base_sv)?;
    let update = bytes("update", &req.update)?;
    let epoch = decimal_u64("epoch", &req.epoch)?;
    let (op, sig, public) = held(req.key, |k| {
        let op = ContentOp::for_key(k, &req.kb_id, &req.node_id, &base_sv, epoch, req.issued_at);
        op.check()?;
        let sig = op.sign_with(k, &update);
        Ok::<_, ApiError>((op, sig, k.public_key()))
    })??;
    /// The ADR-036 header, as the daemon reads it: the top-level params of
    /// `kb/node_update`, or `params.content_header` of `sync/update`.
    #[derive(Serialize)]
    struct Header {
        author: String,
        epoch: String,
        issued_at: u64,
        base_sv: String,
        sig: String,
        author_pubkey: String,
    }
    out(&Header {
        author: op.author,
        epoch: op.epoch.to_string(),
        issued_at: op.issued_at,
        base_sv: STANDARD.encode(&op.base_sv),
        sig: STANDARD.encode(sig),
        author_pubkey: STANDARD.encode(public),
    })
}

/// The fields of an op signed outside the module: `op_canonical_bytes`'s
/// request, and `verify_op`'s with `sig`. The author is named by its public
/// key, and its fingerprint derived from it, never claimed.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutsideOpReq<'a> {
    #[serde(borrow)]
    kb_id: Cow<'a, str>,
    #[serde(borrow)]
    node_id: Cow<'a, str>,
    #[serde(borrow)]
    base_sv: Cow<'a, str>,
    #[serde(borrow)]
    author_pubkey: Cow<'a, str>,
    /// Decimal text, as `sign_op`'s.
    #[serde(borrow)]
    epoch: Cow<'a, str>,
    issued_at: u64,
    #[serde(borrow)]
    update: Cow<'a, str>,
    /// `verify_op` only.
    #[serde(borrow, default)]
    sig: Option<Cow<'a, str>>,
    /// Optional: the author the client would send. It must be the key's
    /// fingerprint (`op_canonical_bytes` refuses another; `verify_op` answers
    /// `valid: false`), as the daemon requires.
    #[serde(borrow, default)]
    author: Option<Cow<'a, str>>,
}

/// The header and payload an outside op names, checked as `sign_op` checks
/// its own: the ids encodable, the epoch canonical, the key 32 bytes.
fn outside_op(req: &OutsideOpReq) -> Result<(ContentOp, [u8; 32], Vec<u8>), ApiError> {
    let base_sv = bytes("base_sv", &req.base_sv)?;
    let update = bytes("update", &req.update)?;
    let epoch = decimal_u64("epoch", &req.epoch)?;
    let public = *bytes32("author_pubkey", &req.author_pubkey)?;
    let op = ContentOp {
        kb_id: req.kb_id.to_string(),
        node_id: req.node_id.to_string(),
        base_sv: base_sv.to_vec(),
        author: member::fingerprint(&public),
        epoch,
        issued_at: req.issued_at,
    };
    op.check()?;
    Ok((op, public, update.to_vec()))
}

/// `op_canonical_bytes` (ADR-098 Amendment 2): the exact bytes ADR-036 D1
/// signs, for a client that signs them with a key it holds outside the module
/// (a non-extractable WebCrypto key), and the header to send with its
/// signature. The same function the daemon verifies against builds them.
fn op_canonical_bytes(r: &[u8]) -> Result<String, ApiError> {
    let req: OutsideOpReq = parse(r)?;
    if req.sig.is_some() {
        return Err(ApiError::new("bad-request", "sig belongs to verify_op"));
    }
    let (op, public, update) = outside_op(&req)?;
    if req.author.as_deref().is_some_and(|a| a != op.author) {
        return Err(ApiError::new(
            "bad-request",
            "author is not author_pubkey's fingerprint",
        ));
    }
    #[derive(Serialize)]
    struct Unsigned {
        canonical: String,
        author: String,
        epoch: String,
        issued_at: u64,
        base_sv: String,
        author_pubkey: String,
    }
    out(&Unsigned {
        canonical: STANDARD.encode(op.canonical_bytes(&update)),
        author: op.author.clone(),
        epoch: op.epoch.to_string(),
        issued_at: op.issued_at,
        base_sv: STANDARD.encode(&op.base_sv),
        author_pubkey: STANDARD.encode(public),
    })
}

/// `verify_op`: whether `sig` is the author's signature over the op, as the
/// daemon checks it. `{valid: false}` for a signature that does not verify; an
/// error only for a request that does not name an op.
fn verify_op(r: &[u8]) -> Result<String, ApiError> {
    let req: OutsideOpReq = parse(r)?;
    let Some(sig) = req.sig.as_deref() else {
        return Err(ApiError::new("bad-request", "verify_op needs sig"));
    };
    let sig = bytes("sig", sig)?;
    let (op, public, update) = outside_op(&req)?;
    #[derive(Serialize)]
    struct Valid {
        valid: bool,
    }
    out(&Valid {
        valid: req.author.as_deref().is_none_or(|a| a == op.author)
            && op.fingerprint_matches(&public)
            && op.verify(&sig, &public, &update),
    })
}
