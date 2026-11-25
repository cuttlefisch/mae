//! What makes a name safe to write as a file on every platform MAE exports to
//! (Linux, macOS and Windows, principle #13), and the key under which two
//! names are the same file there. One definition, for every caller that turns
//! a stored name into a path.

use icu_casemap::CaseMapper;
use unicode_normalization::UnicodeNormalization;

/// Two names that are one file on a case-insensitive or
/// normalization-insensitive filesystem (APFS and NTFS by default, many
/// mounts) share this key: Unicode canonical caseless matching (Unicode
/// §3.13, D145), `NFD(toCasefold(NFD(name)))`, written in NFC.
///
/// `toCasefold` is full default case folding, the C and F mappings of
/// `CaseFolding.txt`, from ICU4X's compiled data. Not `str::to_lowercase`:
/// lowercase keeps `ſ` and `s`, `ς` and `σ`, `ß` and `ss` apart, and folding
/// does not. Not the Turkic (T) mappings either, and no locale anywhere, so
/// every peer computes the same key for the same name: `İ` folds to `i` plus
/// a combining dot (not to `i`), and dotless `ı` folds to itself. Unicode
/// keeps case folding stable for assigned characters, so a peer on an older
/// Unicode release only differs on characters it does not know yet.
///
/// The NFD before folding is what makes folding see a character's decomposed
/// marks (`U+0345` folds to `ι` only once separated); the NFC after it is
/// only the spelling of the key, as NFD and NFC are each one-to-one on
/// canonically equivalent strings.
pub fn fold_key(name: &str) -> String {
    let decomposed: String = name.nfd().collect();
    CaseMapper::new()
        .fold_string(&decomposed)
        .nfc()
        .collect::<String>()
}

/// Length in UTF-8 bytes of `name`'s NFD form: what APFS stores, so the
/// bytes it counts against `NAME_MAX`.
pub fn nfd_len(name: &str) -> usize {
    name.nfd().map(char::len_utf8).sum()
}

/// Unicode general category Cf (format: zero-width and bidirectional
/// controls, soft hyphen, tags, …), from Unicode 15.1
/// `DerivedGeneralCategory.txt`. Invisible, and the bidi controls among them
/// can reorder what a reader sees, so a name carrying one looks like a name
/// without it. Listed by range because `std` has no category lookup: a
/// character added to Cf later would be missed here, never wrongly refused.
pub fn is_format_char(c: char) -> bool {
    const CF: &[(u32, u32)] = &[
        (0x00AD, 0x00AD),
        (0x0600, 0x0605),
        (0x061C, 0x061C),
        (0x06DD, 0x06DD),
        (0x070F, 0x070F),
        (0x0890, 0x0891),
        (0x08E2, 0x08E2),
        (0x180E, 0x180E),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x2064),
        (0x2066, 0x206F),
        (0xFEFF, 0xFEFF),
        (0xFFF9, 0xFFFB),
        (0x110BD, 0x110BD),
        (0x110CD, 0x110CD),
        (0x13430, 0x1343F),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0001, 0xE0001),
        (0xE0020, 0xE007F),
    ];
    let c = c as u32;
    CF.iter().any(|&(lo, hi)| (lo..=hi).contains(&c))
}

/// Characters Windows refuses in a file name (besides `/`, `\` and the
/// controls, which every caller refuses already).
pub const WINDOWS_FORBIDDEN: &[char] = &['<', '>', ':', '"', '|', '?', '*'];

/// Windows' reserved device names. `CON.png` opens the console, not a file.
/// The set is Python's `ntpath.isreserved` (3.13), which follows Microsoft's
/// "Naming Files, Paths, and Namespaces": the superscript-digit ports and the
/// console handles are reserved too.
const WINDOWS_DEVICES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Whether Windows reads `name` as a device, case-insensitively and with or
/// without an extension: the part before the first `.`, trailing spaces
/// removed (`CON .png` is the console too), is a reserved name.
pub fn is_windows_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    WINDOWS_DEVICES.iter().any(|d| d.eq_ignore_ascii_case(stem))
}
