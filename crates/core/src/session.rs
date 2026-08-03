//! Session persistence — save and restore buffer list + cursor positions.
//!
//! Sessions are stored as JSON at `{project_root}/.mae/session.json`.
//! Non-file buffers (shell, AI conversation, help, etc.) are skipped.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Serialized session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub buffers: Vec<SessionBuffer>,
    pub focused_idx: usize,
}

/// The index domain of a persisted `cursor_col` (ADR-087 Rule 4).
///
/// This exists because MAE **changed** that domain: sessions written before
/// the Rule 4 migration hold a *character* column, sessions written after hold
/// a *byte* column. On a line like `日本語のテキスト` the two disagree by a
/// factor of three, so reading an old file as if it were new silently teleports
/// the cursor. The domain is therefore recorded **in the file, per record**,
/// not inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnDomain {
    /// `cursor_col` counts Unicode scalar values from the start of the line.
    ///
    /// The **default**, and deliberately so: a v1 session file has no
    /// `cursor_col_domain` key at all, so `#[serde(default)]` must resolve to
    /// the legacy meaning. Defaulting to `Byte` would make every unlabelled
    /// old file silently wrong — the exact failure this enum prevents.
    #[default]
    Char,
    /// `cursor_col` counts bytes from the start of the line — the ADR-087
    /// Rule 4 domain, what `Window::cursor_col` holds in memory.
    Byte,
}

/// Per-buffer state saved in a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionBuffer {
    pub file_path: PathBuf,
    pub cursor_row: usize,
    /// Cursor column in the domain named by [`SessionBuffer::cursor_col_domain`].
    /// Never assign this to `Window::cursor_col` directly — go through
    /// [`SessionBuffer::cursor_col_as_byte_col`], which needs the line's text
    /// and is why the conversion cannot happen at parse time.
    pub cursor_col: usize,
    /// Which domain `cursor_col` is in. Absent in v1 files, where
    /// `#[serde(default)]` yields [`ColumnDomain::Char`].
    #[serde(default)]
    pub cursor_col_domain: ColumnDomain,
    pub scroll_offset: usize,
    pub project_root: Option<PathBuf>,
}

impl SessionBuffer {
    /// Resolve `cursor_col` to a byte column against the buffer that was
    /// actually opened for `file_path`.
    ///
    /// The conversion is deferred to here — rather than done while parsing —
    /// because char -> byte is not a pure function of the number: it needs the
    /// text of `cursor_row`. A session file stores a path, not content, so the
    /// file on disk may also have changed since the session was written; the
    /// result is snapped to a grapheme boundary and clamped to the line, so a
    /// stale column lands somewhere valid instead of mid-UTF-8.
    pub fn cursor_col_as_byte_col(&self, buf: &crate::buffer::Buffer) -> usize {
        let row = self.cursor_row.min(buf.line_count().saturating_sub(1));
        let byte_col = match self.cursor_col_domain {
            ColumnDomain::Char => buf.char_col_to_byte_col(row, self.cursor_col),
            ColumnDomain::Byte => self.cursor_col,
        };
        buf.snap_col_to_grapheme(row, byte_col.min(buf.line_byte_len(row)))
    }
}

impl Session {
    /// v1: `cursor_col` is a char column, and the `cursor_col_domain` key does
    /// not exist. v2: `cursor_col_domain` is written explicitly and is `byte`.
    ///
    /// v1 files are **read**, not rejected — the alternative is throwing away
    /// a user's whole buffer list over a column-encoding detail. They are
    /// normalised to `Char` on load regardless of what the record claims, so a
    /// hand-edited or partially-upgraded file cannot smuggle a byte column in
    /// under a v1 header.
    pub const VERSION: u32 = 2;
    /// Oldest session format this build can still read.
    pub const MIN_SUPPORTED_VERSION: u32 = 1;

    /// Build a session from current editor state.
    pub fn from_editor(editor: &super::Editor) -> Self {
        let win = editor.window_mgr.focused_window();
        let focused_idx = win.buffer_idx;

        let buffers: Vec<SessionBuffer> = editor
            .buffers
            .iter()
            .enumerate()
            .filter_map(|(i, buf)| {
                // Only save file-backed text buffers
                if buf.kind != crate::BufferKind::Text {
                    return None;
                }
                let file_path = buf.file_path()?.to_path_buf();
                let (cursor_row, cursor_col, scroll_offset) = if i == focused_idx {
                    (win.cursor_row, win.cursor_col, win.scroll_offset)
                } else {
                    // For non-focused buffers, save defaults (we don't track per-buffer cursors easily)
                    (0, 0, 0)
                };
                Some(SessionBuffer {
                    file_path,
                    cursor_row,
                    cursor_col,
                    // `Window::cursor_col` is a byte column (ADR-087 Rule 4),
                    // so that is what we are writing. Recorded, never implied.
                    cursor_col_domain: ColumnDomain::Byte,
                    scroll_offset,
                    project_root: buf.project_root.clone(),
                })
            })
            .collect();

        Session {
            version: Self::VERSION,
            buffers,
            focused_idx,
        }
    }

    /// Session file path for a project root.
    pub fn session_path(project_root: &Path) -> PathBuf {
        project_root.join(".mae").join("session.json")
    }

    /// Save session to disk.
    pub fn save(&self, project_root: &Path) -> Result<(), String> {
        let path = Self::session_path(project_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create .mae dir: {}", e))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize session: {}", e))?;
        std::fs::write(&path, json).map_err(|e| format!("Failed to write session: {}", e))?;
        Ok(())
    }

    /// Load session from disk.
    pub fn load(project_root: &Path) -> Result<Self, String> {
        let path = Self::session_path(project_root);
        let content =
            std::fs::read_to_string(&path).map_err(|e| format!("Failed to read session: {}", e))?;
        let mut session: Session = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse session: {}", e))?;
        if session.version > Self::VERSION || session.version < Self::MIN_SUPPORTED_VERSION {
            return Err(format!(
                "Session version mismatch: this build reads {}..={}, got {}",
                Self::MIN_SUPPORTED_VERSION,
                Self::VERSION,
                session.version
            ));
        }
        session.normalize_column_domains();
        Ok(session)
    }

    /// Force every record's declared column domain to what its *file version*
    /// implies (ADR-087 Rule 4 migration).
    ///
    /// v1 predates the field entirely, so a v1 record claiming `byte` is
    /// either hand-edited or the product of a botched merge; trusting it would
    /// reintroduce exactly the silent reinterpretation this migration exists to
    /// prevent. The file version wins.
    fn normalize_column_domains(&mut self) {
        if self.version >= 2 {
            return;
        }
        for sb in &mut self.buffers {
            sb.cursor_col_domain = ColumnDomain::Char;
        }
        // Upgrade the *header* in memory only. The records keep the honest
        // `Char` label, so a subsequent `save` round-trips them losslessly as
        // v2-with-char-columns rather than relabelling them `byte` and
        // corrupting them.
        self.version = Self::VERSION;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session {
            version: Session::VERSION,
            buffers: vec![SessionBuffer {
                file_path: PathBuf::from("/tmp/test.rs"),
                cursor_row: 10,
                cursor_col: 5,
                cursor_col_domain: ColumnDomain::Byte,
                scroll_offset: 3,
                project_root: Some(PathBuf::from("/tmp")),
            }],
            focused_idx: 0,
        };
        session.save(dir.path()).unwrap();
        let loaded = Session::load(dir.path()).unwrap();
        assert_eq!(loaded.buffers.len(), 1);
        assert_eq!(loaded.buffers[0].cursor_row, 10);
        assert_eq!(loaded.focused_idx, 0);
    }

    #[test]
    fn session_load_missing_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Session::load(dir.path()).is_err());
    }

    // -----------------------------------------------------------------
    // ADR-087 Rule 4 session-file migration.
    //
    // The failure being guarded against is *silent*: a v1 file read as if its
    // char columns were byte columns moves the cursor on every non-ASCII line
    // and reports nothing. These tests therefore assert the resolved position
    // against the text, not against the stored number.
    // -----------------------------------------------------------------

    /// Lines whose char and byte columns disagree, plus one where they agree
    /// (so a test that accidentally passes on ASCII is visibly not enough).
    fn migration_corpus() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ascii-agrees", "fn main() { let x = 1; }"),
            ("cjk", "let 名前 = \"日本語のテキスト\";"),
            (
                "combining",
                "cafe\u{0301} nai\u{0308}ve re\u{0301}sume\u{0301}",
            ),
            ("zwj-emoji", "let team = \"👨‍👩‍👧‍👦\"; // 👍🏽"),
            ("astral-cjk", "\u{20000}\u{2A6B2} rare ideographs"),
            ("mixed", "a日👨‍👩‍👧b\u{0301}c → d"),
        ]
    }

    fn buffer_with(text: &str) -> crate::buffer::Buffer {
        let mut b = crate::buffer::Buffer::new();
        b.replace_contents(text);
        b
    }

    fn write_v1_session(dir: &Path, file_path: &Path, row: usize, char_col: usize) {
        // A literal v1 document: no `cursor_col_domain` key at all. Written as
        // raw JSON rather than by serializing a struct, because the point is
        // to prove MAE can still read what *older builds actually wrote*.
        let json = format!(
            r#"{{
  "version": 1,
  "buffers": [
    {{
      "file_path": {path},
      "cursor_row": {row},
      "cursor_col": {col},
      "scroll_offset": 0,
      "project_root": null
    }}
  ],
  "focused_idx": 0
}}"#,
            path = serde_json::to_string(file_path).unwrap(),
            row = row,
            col = char_col,
        );
        let p = Session::session_path(dir);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, json).unwrap();
    }

    #[test]
    fn a_v1_file_has_no_domain_key_and_defaults_to_char_not_byte() {
        let dir = tempfile::tempdir().unwrap();
        write_v1_session(dir.path(), Path::new("/tmp/x.rs"), 0, 3);
        let loaded = Session::load(dir.path()).unwrap();
        assert_eq!(
            loaded.buffers[0].cursor_col_domain,
            ColumnDomain::Char,
            "an unlabelled legacy column must default to Char; defaulting to \
             Byte is the silent-corruption bug this field exists to prevent"
        );
        // The stored number is untouched — only its interpretation is pinned.
        assert_eq!(loaded.buffers[0].cursor_col, 3);
    }

    #[test]
    fn a_v1_char_column_resolves_to_the_same_visible_position_as_before() {
        for (name, line) in migration_corpus() {
            let buf = buffer_with(line);
            // Walk every char boundary: that is the full set of columns a
            // pre-migration MAE could have persisted.
            for (char_col, (byte_col, _)) in line.char_indices().enumerate() {
                let sb = SessionBuffer {
                    file_path: PathBuf::from("/x"),
                    cursor_row: 0,
                    cursor_col: char_col,
                    cursor_col_domain: ColumnDomain::Char,
                    scroll_offset: 0,
                    project_root: None,
                };
                let resolved = sb.cursor_col_as_byte_col(&buf);
                // The oracle is the *text*, not the number: the resolved byte
                // column must name the same character the old char column did
                // (modulo the grapheme snap, which only ever moves left onto
                // the cluster that character belongs to).
                let expected = crate::grapheme::snap_to_grapheme_boundary(line, byte_col);
                assert_eq!(
                    resolved, expected,
                    "{name}: char col {char_col} (byte {byte_col}) resolved to {resolved}"
                );
            }
        }
    }

    #[test]
    fn reading_a_v1_column_as_a_byte_column_would_have_been_wrong() {
        // The negative case. If this ever stops failing, the migration has
        // become a no-op and the test above is proving nothing.
        let mut disagreements = 0;
        for (_, line) in migration_corpus() {
            let buf = buffer_with(line);
            for (char_col, (byte_col, _)) in line.char_indices().enumerate() {
                let migrated = SessionBuffer {
                    file_path: PathBuf::from("/x"),
                    cursor_row: 0,
                    cursor_col: char_col,
                    cursor_col_domain: ColumnDomain::Char,
                    scroll_offset: 0,
                    project_root: None,
                };
                let naive = SessionBuffer {
                    cursor_col_domain: ColumnDomain::Byte,
                    ..migrated.clone()
                };
                if migrated.cursor_col_as_byte_col(&buf) != naive.cursor_col_as_byte_col(&buf) {
                    disagreements += 1;
                }
                let _ = byte_col;
            }
        }
        assert!(
            disagreements > 0,
            "the naive reinterpretation must differ from the migration on a \
             non-ASCII corpus, or this migration is not doing anything"
        );
    }

    #[test]
    fn a_v2_byte_column_is_passed_through_unchanged() {
        for (name, line) in migration_corpus() {
            let buf = buffer_with(line);
            for (byte_col, _) in line.grapheme_boundaries_for_test() {
                let sb = SessionBuffer {
                    file_path: PathBuf::from("/x"),
                    cursor_row: 0,
                    cursor_col: byte_col,
                    cursor_col_domain: ColumnDomain::Byte,
                    scroll_offset: 0,
                    project_root: None,
                };
                assert_eq!(
                    sb.cursor_col_as_byte_col(&buf),
                    byte_col,
                    "{name}: a byte column at a cluster boundary must round-trip"
                );
            }
        }
    }

    #[test]
    fn a_v1_record_claiming_byte_is_overruled_by_its_file_version() {
        // Hand-edited / badly-merged file: v1 header, but a record asserting
        // the new domain. The header wins, because a v1 *writer* never emitted
        // that key and so cannot have meant it.
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"version":1,"buffers":[{"file_path":"/tmp/x.rs","cursor_row":0,
            "cursor_col":9,"cursor_col_domain":"byte","scroll_offset":0,"project_root":null}],
            "focused_idx":0}"#;
        let p = Session::session_path(dir.path());
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, json).unwrap();
        let loaded = Session::load(dir.path()).unwrap();
        assert_eq!(loaded.buffers[0].cursor_col_domain, ColumnDomain::Char);
    }

    #[test]
    fn a_stale_or_out_of_range_column_lands_somewhere_valid_rather_than_mid_utf8() {
        // The file changed since the session was written — a routine case, not
        // an exotic one. Nothing may produce a mid-sequence offset.
        for (name, line) in migration_corpus() {
            let buf = buffer_with(line);
            for domain in [ColumnDomain::Char, ColumnDomain::Byte] {
                for cursor_col in [0, 1, 2, 3, 7, line.len(), line.len() + 5, usize::MAX / 2] {
                    for cursor_row in [0usize, 1, 99] {
                        let sb = SessionBuffer {
                            file_path: PathBuf::from("/x"),
                            cursor_row,
                            cursor_col,
                            cursor_col_domain: domain,
                            scroll_offset: 0,
                            project_root: None,
                        };
                        let col = sb.cursor_col_as_byte_col(&buf);
                        let row = cursor_row.min(buf.line_count().saturating_sub(1));
                        let text = buf.line_text_no_newline(row);
                        assert!(col <= text.len(), "{name}: {domain:?} col {col} overran");
                        assert!(
                            text.is_char_boundary(col),
                            "{name}: {domain:?} col {col} split a UTF-8 sequence"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_future_version_is_refused_but_v1_and_v2_are_read() {
        let dir = tempfile::tempdir().unwrap();
        for (version, should_load) in [(0u32, false), (1, true), (2, true), (3, false), (99, false)]
        {
            let json = format!(
                r#"{{"version":{version},"buffers":[],"focused_idx":0}}"#,
                version = version
            );
            let p = Session::session_path(dir.path());
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, json).unwrap();
            assert_eq!(
                Session::load(dir.path()).is_ok(),
                should_load,
                "version {version} load expectation"
            );
        }
    }

    /// Test-only helper so the byte-column test iterates real cluster
    /// boundaries without `mae-core`'s tests importing `unicode-segmentation`
    /// directly (ADR-087 Rule 7 keeps that import in `grapheme.rs`).
    trait GraphemeBoundariesForTest {
        fn grapheme_boundaries_for_test(&self) -> Vec<(usize, ())>;
    }
    impl GraphemeBoundariesForTest for str {
        fn grapheme_boundaries_for_test(&self) -> Vec<(usize, ())> {
            let mut out = vec![];
            let mut i = 0;
            while i < self.len() {
                out.push((i, ()));
                i = crate::grapheme::next_grapheme_boundary(self, i);
            }
            out.push((self.len(), ()));
            out
        }
    }
}
