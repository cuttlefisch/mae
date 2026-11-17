//! The durable half of an ADR-040 rotation: the successor key and what is known of each KB's
//! `Rebind`, written BEFORE any `Rebind` leaves the editor and kept until every KB's outcome
//! is settled. A connection can drop after the daemon applied a `Rebind` and before its
//! answer arrives; without this record the editor would forget the only key that KB now
//! accepts.
//!
//! One file, `<collab dir>/rotation_pending`, beside the identity key and held to the same
//! rules: created 0600 (never created wider and narrowed), replaced atomically
//! (`write_secure_atomic`), the directory 0700. It holds a secret, so the record has no
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
    /// key's membership tells which.
    Ambiguous,
    /// The daemon holds it: it answered yes, or the new key is a member.
    Applied,
    /// The daemon answered the original with a refusal, or a refused re-send was followed by
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

/// Write `rec`, replacing any previous record atomically, 0600 from creation.
pub(crate) fn save(dir: &Path, rec: &Record) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let body = serde_json::to_string(rec).map_err(std::io::Error::other)?;
    mae_mcp::keystore::write_secure_atomic(&path(dir), &body)
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
        assert!(!dir.join(format!("{FILE}.tmp")).exists());
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
}
