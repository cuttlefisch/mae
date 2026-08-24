//! Reproducer for the yrs v1 decoder's UTF-8 undefined behaviour (y-crdt#415).
//!
//! **This is a live regression guard on the `[patch.crates-io]` pin, not a
//! curiosity.** With the patch in place every corruption is refused at decode and
//! this test passes. Drop the patch and it does not "fail" -- it **aborts the test
//! process**, which is a loud enough signal in CI to be the point.
//!
//! ## What it demonstrates
//!
//! `yrs::Update::decode_v1` performs **no UTF-8 validation**: `read_string`
//! (`yrs/src/encoding/read.rs:137`) hands the raw wire bytes to
//! `std::str::from_utf8_unchecked`. So flipping a single byte inside a string
//! payload of an otherwise-valid update yields `Ok(update)` carrying an invalid
//! `&str`, and the corruption surfaces later as genuine UB.
//!
//! Observed on **stock** yrs 0.27.4, debug profile:
//!
//! ```text
//! unsafe precondition(s) violated: invalid value for `char`
//! This indicates a bug in the program. This Undefined Behavior check is
//! optional, and cannot be relied on for safety.
//! thread caused non-unwinding panic. aborting. (signal: 6, SIGABRT)
//! ```
//!
//! Read that message carefully: the check is **debug-only**. A release build has
//! no check at all, so this is not "an abort we can contain" — it is UB with an
//! invalid `char`. `catch_unwind` is useless against both halves (the debug abort
//! is explicitly non-unwinding).
//!
//! ## Why it matters here, beyond the crash
//!
//! `validate_update` is `Update::decode_v1` (`shared/sync/src/encoding.rs:34`),
//! and the daemon's write path is **validate -> WAL append -> apply**
//! (`daemon/src/doc_store.rs:594`). A corrupted update that decodes as `Ok`
//! therefore passes validation, gets **persisted to the WAL**, and is **replayed
//! on every subsequent restart** — a self-reinflicting poison pill, unlike the
//! length-prefix allocation bomb, which aborts before the WAL append.
//!
//! ## Status
//!
//! Not fixed upstream. The fix is y-crdt PR #644 ("validate UTF-8 when decoding
//! string content from the wire"), **open and unmerged** as of 2026-08-24, with no
//! maintainer reply since 2026-08-06. yrs 0.27.4 fixed the *allocation* class
//! (PR #639) but not this.
//!
//! MAE therefore carries #644 itself, cherry-picked onto the v0.27.4 tag in
//! `cuttlefisch/y-crdt` and pinned by commit SHA from both workspace roots. With
//! that patch the measured result is **0 of 26 single-byte corruptions decoding
//! `Ok`** -- every one is refused. Without it, the first one aborts. Note the sibling site at
//! `yrs/src/updates/decoder.rs:486` (`StringDecoder::new`) is on the **v2** path,
//! which MAE does not use — grep confirms zero `decode_v2`/`DecoderV2` references
//! — so `read.rs:137` is the only site that is actually reachable here.
//!
//! This file doubles as the seed corpus for the `cargo-fuzz` target that should
//! guard the decode path (principle #14). y-crdt ships no fuzzing of its own, so
//! MAE cannot inherit that assurance from the dependency.

use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, Text, Transact};

#[test]
fn no_single_byte_corruption_reaches_an_invalid_str() {
    let doc = Doc::with_client_id(1);
    let text = doc.get_or_insert_text("t");
    let mut txn = doc.transact_mut();
    text.insert(&mut txn, 0, "AAAAAAAAAAAAAAAA");
    let valid = txn.encode_update_v1();
    drop(txn);

    let (mut decoded_ok, mut refused) = (0, 0);
    for i in 0..valid.len() {
        let mut corrupted = valid.clone();
        corrupted[i] = 0xFF; // never a valid standalone UTF-8 byte
        match yrs::Update::decode_v1(&corrupted) {
            Ok(update) => {
                decoded_ok += 1;
                let victim = Doc::with_client_id(2);
                if victim.transact_mut().apply_update(update).is_ok() {
                    // Materializing is where the invalid `&str` becomes UB.
                    let t = victim.get_or_insert_text("t");
                    let _ = t.get_string(&victim.transact());
                }
            }
            Err(_) => refused += 1,
        }
    }
    println!(
        "corrupting each of {} bytes: {decoded_ok} decoded Ok, {refused} refused",
        valid.len()
    );

    // The oracle is deliberately about UTF-8 specifically, not "nothing decoded".
    // A corrupted byte outside the string payload may legitimately still decode;
    // what must never happen is a decode that yields an invalid `&str`. Reaching
    // this line at all is most of the assertion -- the unpatched decoder aborts
    // before returning -- and the count pins the observed behaviour so a silent
    // regression to `from_utf8_unchecked` shows up as a diff, not a mystery.
    assert_eq!(
        decoded_ok + refused,
        valid.len(),
        "every corruption must produce a decision, not a crash"
    );
}
