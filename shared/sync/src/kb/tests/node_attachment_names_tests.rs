//! ADR-115 D4's NAME rule is one portable file name on Linux, macOS and
//! Windows, and no two NAMEs of one node may be one file there; a pair two
//! peers make concurrently converges and both are kept. The sweep's reader
//! counts what the display reader rejects. The field-type check refuses each
//! new bad entry, however many are already there.

use std::path::Path;

use super::*;
use yrs::MapPrelim;

// --- the NAME rule ---------------------------------------------------------

/// Whether `name` is refused for the reason containing `why`.
fn refused_for(name: &str, why: &str) -> bool {
    matches!(validate_attachment_name(name), Err(AttachmentError::Name(w)) if w.contains(why))
}

#[test]
fn the_name_rule_on_named_cases() {
    for ok in [
        "a.png",
        "ñandú.jpg",
        "v1..2.png",
        ".hidden",
        "a b.png",
        " lead.png",
        "😀",
        "x;y.png",
        "CONSOLE.png",
        "COM10.png",
        "LPT.png",
        "a.CON",
        "con1.png",
        &"x".repeat(255),
    ] {
        assert_eq!(validate_attachment_name(ok), Ok(()), "{ok:?}");
    }
    let long = "x".repeat(256);
    let long_utf8 = "ñ".repeat(128); // 255 BYTES, not characters.
    let mut bad: Vec<(&str, &str)> = vec![
        ("", "empty"),
        (".", "directory"),
        ("..", "directory"),
        ("a/b", "separator"),
        ("/abs", "separator"),
        ("a\\b", "separator"),
        ("a\nb", "control"),
        ("a\u{7f}", "control"),
        ("a\u{85}b", "control"),
        ("\u{0}", "control"),
        (&long, "longer"),
        (&long_utf8, "longer"),
        // Unicode Cf: bidi controls, zero-width, soft hyphen, BOM, tags.
        ("a\u{200B}b.png", "format"),
        ("\u{202E}gnp.exe", "format"),
        ("a\u{2066}b.png", "format"),
        ("a\u{AD}b.png", "format"),
        ("\u{FEFF}a.png", "format"),
        ("a\u{E0041}.png", "format"),
        // A trailing dot or space, which Windows strips.
        ("a.png.", "ends in"),
        ("a.png ", "ends in"),
        ("...", "ends in"),
        (" ", "ends in"),
    ];
    let forbidden: Vec<String> = ['<', '>', ':', '"', '|', '?', '*']
        .iter()
        .map(|c| format!("a{c}b.png"))
        .collect();
    bad.extend(forbidden.iter().map(|n| (n.as_str(), "Windows refuses")));
    for device in [
        "CON",
        "con",
        "con.png",
        "Con.tar.gz",
        "CON .png",
        "NUL.txt",
        "aux",
        "PRN.md",
        "com1",
        "COM9.png",
        "lpt1.txt",
        "LPT9",
        "com\u{b9}.png",
        "CONIN$",
        "conout$.log",
    ] {
        bad.push((device, "device name"));
    }
    for (name, why) in bad {
        assert!(
            refused_for(name, why),
            "{name:?}: want refused for {why:?}, got {:?}",
            validate_attachment_name(name)
        );
    }
}

/// An oracle written apart from the rule: the device names Windows reserves
/// (Python's `ntpath.isreserved`), and the invisible characters the generator
/// below can produce.
const DEVICES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5",
    "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];
const INVISIBLE: &[char] = &['\u{200B}', '\u{202E}', '\u{AD}'];

/// Whether `name` is one file, directly in its directory, with the same name
/// on Linux, macOS and Windows.
fn portable_file_name(name: &str) -> bool {
    let dir = Path::new("/attach/dir");
    let joined = dir.join(name);
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    joined.parent() == Some(dir)
        && joined.file_name().and_then(|f| f.to_str()) == Some(name)
        && name.len() <= MAX_ATTACHMENT_NAME_BYTES
        && unicode_normalization::UnicodeNormalization::nfd(name)
            .collect::<String>()
            .len()
            <= MAX_ATTACHMENT_NAME_BYTES
        && !name
            .chars()
            .any(|c| c.is_control() || "\\<>:\"|?*".contains(c) || INVISIBLE.contains(&c))
        && !name.ends_with(['.', ' '])
        && !DEVICES.contains(&stem.as_str())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4096))]

    /// An accepted NAME is exactly one portable file name, directly inside
    /// the attachment directory, on every platform an export runs on; a
    /// refused one is not.
    #[test]
    fn an_accepted_name_is_one_file_in_its_directory(
        parts in prop::collection::vec(
            prop::sample::select(&[
                "a", ".", "/", "\\", "\u{0}", "\n", "\u{85}", "ñ", "😀", " ", ":", "<", ">",
                "|", "?", "*", "\"", "\u{200B}", "\u{202E}", "\u{AD}", "con", "Com1", "lpt3",
                "NUL", "aux", "prn", "com\u{b9}", "conin$",
            ][..]),
            0..8,
        ),
        pad in 0usize..3,
    ) {
        let name = format!("{}{}", parts.concat(), "x".repeat(pad * 127));
        let accepted = validate_attachment_name(&name).is_ok();
        prop_assert_eq!(accepted, portable_file_name(&name), "{:?}", name);
    }
}

// --- one file per NAME -----------------------------------------------------

/// Pairs that are one file on APFS or NTFS: case, NFC against NFD, both.
const SAME_FILE: &[(&str, &str)] = &[
    ("Photo.png", "photo.png"),
    ("café.png", "cafe\u{301}.png"),
    ("CAFÉ.PNG", "cafe\u{301}.png"),
    ("Ä.png", "ä.png"),
];

#[test]
fn a_writer_refuses_a_second_name_for_the_same_file() {
    for &(first, second) in SAME_FILE {
        let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
        let _ = n.set_attachment(first, &png(1)).unwrap();
        let _ = n.set_attachment("other.png", &png(3)).unwrap();
        let sv = n.state_vector();
        let collides = Err(AttachmentError::NameCollides {
            existing: first.to_string(),
        });
        assert_eq!(n.set_attachment(second, &png(2)), collides, "{second:?}");
        assert_eq!(n.rename_attachment("other.png", second), collides);
        assert_eq!(n.state_vector(), sv, "a refused write wrote");
        // Renaming a NAME to another spelling of itself is not a second file.
        let _ = n.rename_attachment(first, second).unwrap();
        assert_eq!(n.attachment(second), Some(png(1)));
        assert!(n.attachment(first).is_none());
    }
    // Names that differ by more than case and normalization coexist.
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    for name in [
        "a.png",
        "b.png",
        "a.jpg",
        "\u{DF}.png",
        "\u{131}.png",
        "i.png",
    ] {
        let _ = n.set_attachment(name, &png(1)).unwrap();
    }
    assert_eq!(n.attachments().len(), 6);
}

/// Full default case folding (`CaseFolding.txt` C+F), not lowercase: these
/// pairs are one key, though NFC then `to_lowercase` keeps each apart.
const FOLD_SAME: &[(&str, &str)] = &[
    ("\u{17F}.png", "s.png"),        // LONG S
    ("\u{3C2}.png", "\u{3C3}.png"),  // final and medial sigma
    ("\u{DF}.png", "ss.png"),        // SHARP S
    ("\u{1E9E}.png", "SS.png"),      // CAPITAL SHARP S
    ("\u{FB01}.png", "fi.png"),      // LIGATURE FI
    ("\u{149}.png", "\u{2BC}n.png"), // N PRECEDED BY APOSTROPHE
];

/// Turkish dotted and dotless I, by default (non-Turkic) folding: `I` is
/// `i`, `İ` is `i` plus a combining dot above (so not `i`), and `ı` is only
/// itself. These pairs are different keys.
const FOLD_DIFFERENT: &[(&str, &str)] = &[
    ("\u{131}.png", "i.png"), // DOTLESS I
    ("\u{131}.png", "I.png"),
    ("\u{130}.png", "i.png"), // I WITH DOT ABOVE
    ("\u{130}.png", "I.png"),
];

#[test]
fn names_fold_by_unicode_default_case_folding() {
    use crate::portable_name::fold_key;
    use unicode_normalization::UnicodeNormalization;
    for &(a, b) in FOLD_SAME {
        assert_eq!(fold_key(a), fold_key(b), "{a:?} {b:?}");
        let lowercase = |s: &str| s.nfc().collect::<String>().to_lowercase();
        assert_ne!(
            lowercase(a),
            lowercase(b),
            "{a:?} {b:?} is no test of folding"
        );
        let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
        let _ = n.set_attachment(a, &png(1)).unwrap();
        assert_eq!(
            n.set_attachment(b, &png(2)),
            Err(AttachmentError::NameCollides { existing: a.into() }),
            "{b:?}"
        );
    }
    for &(a, b) in FOLD_DIFFERENT {
        assert_ne!(fold_key(a), fold_key(b), "{a:?} {b:?}");
    }
    assert_eq!(fold_key("I.png"), fold_key("i.png"));
    // The key itself, spelled out: the same on every peer, as nothing it is
    // computed from is a locale or a setting.
    for (name, key) in [
        ("\u{130}", "i\u{307}"),
        ("\u{131}", "\u{131}"),
        ("I", "i"),
        ("\u{DF}", "ss"),
        ("\u{17F}", "s"),
        ("\u{3C2}", "\u{3C3}"),
        ("\u{212B}", "\u{E5}"), // ANGSTROM SIGN: decomposed, folded, composed
        ("CAFE\u{301}", "café"),
    ] {
        assert_eq!(fold_key(name), key, "{name:?}");
    }
}

/// The 255-byte limit holds for the NFD form too, which APFS stores: a name
/// of precomposed characters can fit as written and not as stored.
#[test]
fn the_byte_limit_counts_the_decomposed_form() {
    // `é` is 2 bytes in NFC and 3 in NFD.
    let fits = "é".repeat(85); // 170 bytes, 255 decomposed
    let over = "é".repeat(86); // 172 bytes, 258 decomposed
    assert_eq!(validate_attachment_name(&fits), Ok(()));
    assert!(over.len() <= MAX_ATTACHMENT_NAME_BYTES);
    assert!(
        refused_for(&over, "decomposed"),
        "{:?}",
        validate_attachment_name(&over)
    );
    // Hangul syllables are 3 bytes composed, 6 or 9 decomposed.
    let hangul = "한".repeat(30); // 90 bytes, 270 decomposed
    assert!(refused_for(&hangul, "decomposed"));
    let ascii = "x".repeat(MAX_ATTACHMENT_NAME_BYTES);
    assert_eq!(validate_attachment_name(&ascii), Ok(()));
    let mut n = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    assert!(n.set_attachment(&over, &png(1)).is_err());
    assert!(n.set_attachment(&fits, &png(1)).is_ok());
}

/// Two NAMEs for one file, each valid where it was written, made by peers
/// that cannot see each other: every peer converges on the same map, and the
/// reader keeps both rather than picking one. A writer then adds no third
/// spelling, but may still rebind or rename one it has.
#[test]
fn a_concurrent_collision_converges_everywhere_and_the_reader_keeps_both() {
    let spellings = ["Photo.png", "photo.png", "PHOTO.PNG"];
    for n in [3usize, 5] {
        for seed in 0..40u64 {
            let maps = converge(n, seed, |i, p| {
                let name = spellings
                    .get(i)
                    .map_or_else(|| format!("p{i}.png"), |s| s.to_string());
                p.set_attachment(&name, &png(i as u8)).unwrap()
            });
            let mut expected: BTreeMap<String, AttachmentRef> = (0..n)
                .map(|i| {
                    let name = spellings
                        .get(i)
                        .map_or_else(|| format!("p{i}.png"), |s| s.to_string());
                    (name, png(i as u8))
                })
                .collect();
            expected.insert("keep.png".into(), png(200));
            expected.insert("drop.png".into(), png(201));
            for m in &maps {
                assert_eq!(m, &expected, "n={n} seed={seed}");
            }
        }
    }

    let base = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1).encode();
    let mut a = KbNodeDoc::from_bytes_with_client_id(&base, 10).unwrap();
    let mut b = KbNodeDoc::from_bytes_with_client_id(&base, 20).unwrap();
    let ua = a.set_attachment("Photo.png", &png(1)).unwrap();
    let ub = b.set_attachment("photo.png", &png(2)).unwrap();
    a.apply_update(&ub).unwrap();
    b.apply_update(&ua).unwrap();
    assert_eq!(a.attachments(), b.attachments());
    assert_eq!(a.version(), b.version());
    assert_eq!(a.attachments().len(), 2, "the reader dropped one spelling");
    assert!(a.read_attachments().problems.is_empty());
    assert_eq!(
        a.set_attachment("PHOTO.png", &png(3)),
        Err(AttachmentError::NameCollides {
            existing: "Photo.png".into()
        })
    );
    let _ = a.set_attachment("photo.png", &png(4)).unwrap();
    assert_eq!(a.attachment("photo.png"), Some(png(4)));
    let _ = a.rename_attachment("photo.png", "photo-2.png").unwrap();
    assert_eq!(a.attachments().len(), 2);
}

/// A node holding a pair two peers bound concurrently is reborn (ADR-107)
/// with both: rebirth copies the converged map, it does not re-author it
/// through the collision rule, so no image is lost and the hash the signed
/// `Rebirth` op carries still matches the original's.
#[test]
fn rebirth_keeps_a_concurrent_collision_pair() {
    let base = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1).encode();
    let mut a = KbNodeDoc::from_bytes_with_client_id(&base, 10).unwrap();
    let mut b = KbNodeDoc::from_bytes_with_client_id(&base, 20).unwrap();
    let ua = a.set_attachment("café.png", &png(1)).unwrap();
    let _ = b.set_attachment("CAFE\u{301}.png", &png(2)).unwrap();
    let _ = b.set_attachment("other.png", &png(3)).unwrap();
    a.apply_update(&b.encode_diff(&a.state_vector()).unwrap())
        .unwrap();
    b.apply_update(&ua).unwrap();
    assert_eq!(a.attachments().len(), 3);
    let reborn = a.reborn(99).unwrap();
    assert_eq!(reborn.attachments(), a.attachments());
    assert_eq!(reborn.rebirth_hash(), a.rebirth_hash());
    assert_eq!(reborn.version(), b.version());
}

// --- the sweep's reader ----------------------------------------------------

/// An update from another client putting a nested shared map at `name`, a
/// shape no MAE writer makes but a peer could.
fn nested(base: &[u8], name: &str, hash: &str) -> Vec<u8> {
    let doc = KbNodeDoc::from_bytes_with_client_id(base, 667).unwrap();
    let sv = doc.state_vector();
    let map = doc.doc().get_or_insert_map(ATTACHMENTS_ROOT);
    {
        let mut txn = doc.doc().transact_mut();
        map.insert(
            &mut txn,
            name,
            MapPrelim::from([("sha256", Any::from(hash))]),
        );
    }
    doc.encode_diff(&sv).unwrap()
}

/// Every entry the display reader rejects but which binds a hash a peer may
/// still need is counted by the sweep's reader; an entry with no hash adds
/// nothing, and the valid entry is always there.
#[test]
fn the_sweep_reader_counts_what_the_display_reader_rejects() {
    let mut honest = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    assert!(honest.referenced_hashes().unwrap().is_empty());
    let _ = honest.set_attachment("ok.png", &png(1)).unwrap();
    let base = honest.encode();
    let upper = sha(0xab).to_uppercase();
    let cases: Vec<(&str, Vec<u8>, Option<String>)> = vec![
        (
            "a bad NAME",
            hostile(&base, "a/b.png", entry(&good_fields())),
            Some(sha(9)),
        ),
        (
            "a device NAME",
            hostile(&base, "CON.png", entry(&good_fields())),
            Some(sha(9)),
        ),
        (
            "a size stored as a string",
            hostile(&base, "s.png", with("size", Some(Any::from("5")))),
            Some(sha(9)),
        ),
        (
            "no media type",
            hostile(&base, "m.png", with("media_type", None)),
            Some(sha(9)),
        ),
        (
            "an uppercase hash",
            hostile(
                &base,
                "u.png",
                with("sha256", Some(Any::from(upper.as_str()))),
            ),
            Some(sha(0xab)),
        ),
        (
            "a nested shared map",
            nested(&base, "y.png", &sha(9)),
            Some(sha(9)),
        ),
        (
            "a hash that is not hex",
            hostile(
                &base,
                "z.png",
                with("sha256", Some(Any::from("z".repeat(64).as_str()))),
            ),
            None,
        ),
        (
            "no map at all",
            hostile(&base, "x.png", Any::from(sha(9).as_str())),
            Some(sha(9)),
        ),
    ];
    for (what, update, hash) in cases {
        let mut peer = KbNodeDoc::from_bytes(&base).unwrap();
        peer.apply_update(&update).unwrap();
        assert_eq!(
            peer.attachments().len(),
            1,
            "{what}: the display reader took it"
        );
        let mut expected: std::collections::BTreeSet<String> = [sha(1)].into();
        expected.extend(hash);
        assert_eq!(peer.referenced_hashes().unwrap(), expected, "{what}");
    }
    let old = KbNodeDoc::from_bytes(&before_bytes()).unwrap();
    assert!(old.referenced_hashes().unwrap().is_empty());
}

// --- the field-type check, per entry ---------------------------------------

#[test]
fn a_second_bad_entry_is_refused_like_the_first() {
    let mut honest = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let _ = honest.set_attachment("ok.png", &png(1)).unwrap();
    let clean = honest.encode();
    let mut legacy = KbNodeDoc::from_bytes(&clean).unwrap();
    legacy
        .apply_update(&hostile(&clean, "m1", with("size", None)))
        .unwrap();
    let base = legacy.encode();
    assert!(legacy.field_type_violations().contains("attachments"));

    let introduced = |update: Vec<u8>| KbNodeDoc::field_type_introduced(&base, &update).unwrap();
    for (what, update) in [
        (
            "a second bad shape",
            hostile(&base, "m2", with("sha256", None)),
        ),
        (
            "a bad NAME",
            hostile(&base, "a/b.png", entry(&good_fields())),
        ),
        (
            "a device NAME",
            hostile(&base, "nul.png", entry(&good_fields())),
        ),
        (
            "a good entry spoilt",
            hostile(&base, "ok.png", with("size", None)),
        ),
    ] {
        assert_eq!(introduced(update), Some("attachments"), "{what}");
    }
    // The legacy document stays writable: touching only its bad entry, or
    // adding a good one, introduces nothing.
    for (what, update) in [
        (
            "the bad entry rewritten",
            hostile(&base, "m1", with("media_type", None)),
        ),
        (
            "a good entry added",
            hostile(&base, "new.png", entry(&good_fields())),
        ),
    ] {
        assert_eq!(introduced(update), None, "{what}");
    }
}

/// A replacement (rebirth, reseal) is judged against the document it
/// replaces: an entry that document already held is not introduced, one it
/// did not hold is, and an old document that does not decode excuses nothing.
#[test]
fn a_replacement_is_judged_against_the_document_it_replaces() {
    let mut honest = KbNodeDoc::new_with_client_id("n", "T", "B", &[], 1);
    let _ = honest.set_attachment("ok.png", &png(1)).unwrap();
    let clean = honest.encode();
    let mut legacy = KbNodeDoc::from_bytes(&clean).unwrap();
    legacy
        .apply_update(&hostile(&clean, "CON.png", entry(&good_fields())))
        .unwrap();
    let existing = legacy.encode();
    let reborn = legacy.reborn(7).unwrap();
    let replacing = |old: &[u8], new: &KbNodeDoc| {
        KbNodeDoc::field_type_introduced_replacing(old, &new.encode_state()).unwrap()
    };
    assert_eq!(replacing(&existing, &reborn), None);
    assert_eq!(replacing(&clean, &reborn), Some("attachments"));
    assert_eq!(replacing(b"not a document", &reborn), Some("attachments"));
    let mut more = KbNodeDoc::from_bytes(&reborn.encode()).unwrap();
    more.apply_update(&hostile(&reborn.encode(), "nul.png", entry(&good_fields())))
        .unwrap();
    assert_eq!(replacing(&existing, &more), Some("attachments"));
    assert_eq!(
        KbNodeDoc::field_type_introduced(&[], &reborn.encode_state()).unwrap(),
        Some("attachments"),
        "judged against nothing, the legacy entry reads as introduced"
    );
}
