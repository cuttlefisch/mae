//! Reproducer for the yrs v1 decoder's UTF-8 undefined behaviour (y-crdt#415).
//!
//! **`#[ignore]`d on purpose: this test does not fail, it ABORTS the process.**
//! Run it deliberately with:
//!
//! ```text
//! cargo test -p mae-sync --test yrs_decoder_utf8_ub_reproducer -- --ignored --nocapture
//! ```
//!
//! ## What it demonstrates
//!
//! `yrs::Update::decode_v1` performs **no UTF-8 validation**: `read_string`
//! (`yrs/src/encoding/read.rs:137`) hands the raw wire bytes to
//! `std::str::from_utf8_unchecked`. So flipping a single byte inside a string
//! payload of an otherwise-valid update yields `Ok(update)` carrying an invalid
//! `&str`, and the corruption surfaces later as genuine UB.
//!
//! Observed on yrs 0.27.4, debug profile:
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
//! (PR #639) but not this. Note the sibling site at
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
#[ignore = "aborts the process by design — this reproduces UB, it does not assert against it"]
fn a_single_corrupted_byte_decodes_as_ok_and_yields_an_invalid_str() {
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
}
