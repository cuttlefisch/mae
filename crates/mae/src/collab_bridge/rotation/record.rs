//! The durable half of an ADR-040 rotation: the successor key and what is known of each KB's
//! `Rebind`, written BEFORE any `Rebind` leaves the editor and kept until every KB's outcome
//! is settled. A connection can drop after the daemon applied a `Rebind` and before its
//! answer arrives; without this record the editor would forget the only key that KB now
//! accepts.
//!
//! One file, `<collab dir>/rotation_pending`, beside the identity key and held to the same
//! rules: created 0600 (never created wider and narrowed), the directory 0700. It is
//! CREATED with no-replace semantics ([`create`]): a record already on disk — even one this
//! run could not use — may be the only copy of a key a daemon accepted, and of two editors
//! sharing the directory only one may start a rotation. The owner's later updates replace it
//! atomically ([`save`]). It holds a secret, so the record has no
//! `Debug`, and nothing here logs or formats `new_key`.
//!
//! @ai-caution: [collab-identity] Delete this file only once no KB can still hold a `Rebind`
//! to `new_key` that the editor has not adopted: every KB `Refused`, or the identity file now
//! holding `new_key` and every re-wrap sent. Deleting it earlier can lock a seat out.

use std::path::{Path, PathBuf};

use mae_mcp::identity::Identity;
use serde::{Deserialize, Serialize};

/// File name of the record, in the collab directory.
pub(crate) const FILE: &str = "rotation_pending";
/// Format version.
const VERSION: u32 = 1;

/// What the editor knows about one KB's `Rebind`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// Sent, or about to be, and not answered. The daemon may or may not hold it.
    Unknown,
    /// Re-sent after a lost answer, and the re-send refused. Either the daemon applied the
    /// first copy (so the old key, now retired, gets nothing) or it never will; only the new
    /// key's membership tells which. Also: the original answered with an error the daemon
    /// may return after applying it (an internal or storage fault), not a refusal it makes
    /// before applying anything.
    Ambiguous,
    /// The daemon holds it: it answered yes, or the new key is a member.
    Applied,
    /// The daemon answered the original with a refusal it makes before applying anything, or a refused re-send was followed by
    /// the daemon saying the new key is not a member.
    Refused,
}

/// One KB of the rotation.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct KbEntry {
    pub kb_id: String,
    /// The `Rebind` deltas (base64 yrs updates), signed by the OLD key. Re-sent byte for
    /// byte, so a re-send can only ever be the same op.
    pub rebind: Vec<String>,
    /// Deltas only the NEW key's session may send once it is the session (the owner's self
    /// re-wrap), base64.
    pub on_successor: Vec<String>,
    pub outcome: Outcome,
    /// The daemon's reason, for a `Refused` KB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A rotation in flight. `switched`: the identity file already holds `new_key`; what is left
/// is the `on_successor` deltas of the `Applied` KBs.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Record {
    v: u32,
    pub old_fp: String,
    pub new_fp: String,
    label: String,
    /// The successor's Ed25519 seed, hex. SECRET.
    new_key: String,
    pub switched: bool,
    pub kbs: Vec<KbEntry>,
}

impl Record {
    pub(crate) fn new(old_fp: &str, new_id: &Identity, kbs: Vec<KbEntry>) -> Self {
        Self {
            v: VERSION,
            old_fp: old_fp.to_string(),
            new_fp: new_id.fingerprint(),
            label: new_id.label().to_string(),
            new_key: hex::encode(new_id.secret_bytes()),
            switched: false,
            kbs,
        }
    }

    /// The successor key, or `None` when the record does not hold the key it names (a
    /// corrupt or hand-edited file): never adopt a key whose fingerprint was not recorded.
    pub(crate) fn new_identity(&self) -> Option<Identity> {
        let bytes: [u8; 32] = hex::decode(&self.new_key).ok()?.try_into().ok()?;
        let id = Identity::from_seed(&bytes, &self.label);
        (id.fingerprint() == self.new_fp).then_some(id)
    }

    pub(crate) fn any(&self, o: Outcome) -> bool {
        self.kbs.iter().any(|k| k.outcome == o)
    }
}

pub(crate) fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Start a record: write `rec`, 0600 from creation, failing (`AlreadyExists`) if anything is
/// already at the record's path. Never replaces.
pub(crate) fn create(dir: &Path, rec: &Record) -> std::io::Result<()> {
    let body = prepare(dir, rec)?;
    mae_mcp::keystore::create_secure_atomic(&path(dir), &body)
}

/// Update the record this rotation created: replace it atomically, 0600 from creation.
pub(crate) fn save(dir: &Path, rec: &Record) -> std::io::Result<()> {
    let body = prepare(dir, rec)?;
    mae_mcp::keystore::write_secure_atomic(&path(dir), &body)
}

/// Whether anything is at the record's path — a file of any content, or a symlink. An
/// error other than "not there" counts as there: a record that cannot even be looked at
/// cannot be ruled out.
pub(crate) fn present(dir: &Path) -> bool {
    !matches!(std::fs::symlink_metadata(path(dir)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

fn prepare(dir: &Path, rec: &Record) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    serde_json::to_string(rec).map_err(std::io::Error::other)
}

/// The record in `dir`; `Ok(None)` when there is none.
pub(crate) fn load(dir: &Path) -> std::io::Result<Option<Record>> {
    match std::fs::read_to_string(path(dir)) {
        Ok(s) => serde_json::from_str::<Record>(&s)
            .map(Some)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn remove(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Slot;
    use super::*;

    fn entry(outcome: Outcome, on_successor: Vec<String>) -> KbEntry {
        KbEntry {
            kb_id: "kb".to_string(),
            rebind: vec![mae_sync::encoding::update_to_base64(&[1, 2, 3])],
            on_successor,
            outcome,
            reason: None,
        }
    }

    fn keys() -> (Identity, Identity) {
        (
            Identity::from_seed(&[11u8; 32], "me"),
            Identity::from_seed(&[12u8; 32], "me"),
        )
    }

    /// The record holds a secret: owner-only from creation, its directory 0700, no
    /// temporary left behind, and it round-trips the key it names.
    #[cfg(unix)]
    #[test]
    fn the_record_is_owner_only_and_holds_the_key_it_names() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("collab");
        let (old, new) = keys();
        save(
            &dir,
            &Record::new(
                &old.fingerprint(),
                &new,
                vec![entry(Outcome::Unknown, vec![])],
            ),
        )
        .unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path(&dir)), 0o600);
        assert_eq!(mode(&dir), 0o700);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from(FILE)],
            "no temporary left behind"
        );
        let back = load(&dir).unwrap().expect("the record");
        let id = back.new_identity().expect("the key");
        assert_eq!(id.fingerprint(), new.fingerprint());
        assert_eq!(id.secret_bytes(), new.secret_bytes());
    }

    /// A record whose key does not match the fingerprint it names (corrupt or tampered) is
    /// never adopted, and never deleted: it may be the only copy of a key.
    #[test]
    fn a_record_whose_key_is_not_the_one_it_names_is_never_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = keys();
        let mut rec = Record::new(
            &old.fingerprint(),
            &new,
            vec![entry(Outcome::Unknown, vec![])],
        );
        rec.new_fp = Identity::from_seed(&[13u8; 32], "x").fingerprint();
        assert!(rec.new_identity().is_none());
        save(tmp.path(), &rec).unwrap();
        assert!(!Slot::load(tmp.path().to_path_buf(), &old).is_pending());
        assert!(path(tmp.path()).exists(), "left in place");
    }

    /// Start-up, by the identity the editor starts with: the old key resumes the rotation,
    /// a stranger's leaves it alone, and the new key (committed) resumes only to send what is
    /// left — or removes the record when nothing is.
    #[test]
    fn start_up_resumes_by_the_identity_it_starts_with() {
        let (old, new) = keys();
        let stranger = Identity::from_seed(&[14u8; 32], "me");
        let save_rec = |kbs| {
            let tmp = tempfile::tempdir().unwrap();
            save(tmp.path(), &Record::new(&old.fingerprint(), &new, kbs)).unwrap();
            tmp
        };

        let t = save_rec(vec![entry(Outcome::Unknown, vec![])]);
        assert!(Slot::load(t.path().to_path_buf(), &old).is_pending());

        let t = save_rec(vec![entry(Outcome::Unknown, vec![])]);
        assert!(!Slot::load(t.path().to_path_buf(), &stranger).is_pending());
        assert!(
            path(t.path()).exists(),
            "another identity's record is left alone"
        );

        let t = save_rec(vec![entry(Outcome::Applied, vec![])]);
        assert!(!Slot::load(t.path().to_path_buf(), &new).is_pending());
        assert!(
            !path(t.path()).exists(),
            "a completed rotation's record is removed"
        );

        let rewrap = mae_sync::encoding::update_to_base64(&[4, 5]);
        let t = save_rec(vec![entry(Outcome::Applied, vec![rewrap])]);
        let slot = Slot::load(t.path().to_path_buf(), &new);
        assert!(slot.is_pending(), "a re-wrap is still to be sent");
        assert!(slot.wake_at().is_none(), "nothing left to ask the daemon");
    }

    /// Finding 1: a record start-up could not use (corrupt here) may be the only copy of a
    /// key the daemon accepted. A new rotation is refused while ANY record is on disk, the
    /// refusal names the file without showing it, and the file is left byte for byte.
    #[test]
    fn a_new_rotation_is_refused_over_an_unusable_record() {
        let (old, new) = keys();
        let unusable: [(&str, Vec<u8>); 3] = [
            ("corrupt", b"{\"v\":1,\"new_key\":\"deadbeefcafe".to_vec()),
            ("empty", Vec::new()),
            ("another identity's", {
                let other = Identity::from_seed(&[15u8; 32], "me");
                let rec = Record::new(
                    &other.fingerprint(),
                    &new,
                    vec![entry(Outcome::Unknown, vec![])],
                );
                serde_json::to_vec(&rec).unwrap()
            }),
        ];
        for (what, bytes) in unusable {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(path(tmp.path()), &bytes).unwrap();
            let mut slot = Slot::load(tmp.path().to_path_buf(), &old);
            assert!(!slot.is_pending(), "{what}: not used at start-up");
            let fresh = std::sync::Arc::new(Identity::from_seed(&[16u8; 32], "me"));
            let err = super::super::begin(&mut slot, &old.fingerprint(), fresh, Vec::new(), 0)
                .expect_err(what);
            assert!(
                err.contains(&path(tmp.path()).display().to_string()),
                "{what}: {err}"
            );
            assert!(
                !err.contains("new_key") && !err.contains("deadbeefcafe"),
                "{what}: {err}"
            );
            assert_eq!(
                std::fs::read(path(tmp.path())).unwrap(),
                bytes,
                "{what}: overwritten"
            );
            assert!(!slot.is_pending(), "{what}");
        }
    }

    /// Two editors on one collab directory: the second create finds the first's record and
    /// fails, and the first record — and its key — is left intact.
    #[test]
    fn a_second_create_leaves_the_first_record_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let (old, new) = keys();
        let first = Record::new(
            &old.fingerprint(),
            &new,
            vec![entry(Outcome::Unknown, vec![])],
        );
        create(tmp.path(), &first).unwrap();
        let before = std::fs::read(path(tmp.path())).unwrap();
        let other = Identity::from_seed(&[17u8; 32], "me");
        let second = Record::new(
            &old.fingerprint(),
            &other,
            vec![entry(Outcome::Unknown, vec![])],
        );
        let e = create(tmp.path(), &second).expect_err("a second create");
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(path(tmp.path())).unwrap(), before);
        let kept = load(tmp.path()).unwrap().unwrap().new_identity().unwrap();
        assert_eq!(kept.fingerprint(), new.fingerprint());
    }
}
