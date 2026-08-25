//! Daily journal notes: path resolution, stub creation, chain-fill, and
//! navigation (today/yesterday/prev/next/goto-date).

use super::*;

/// Where this KB's dailies live.
///
/// **Phase 3.** Dailies were unconditionally file-backed: existence was
/// `path.exists()`, creation was `fs::write`, and chain-fill string-spliced
/// `Previous:`/`Next:` into file text. Post-cutover that is *wholly broken*, not
/// degraded — a detached KB has no `.org` directory for any of it to touch, and a
/// fresh install has none either.
///
/// The algorithm is the same either way; only the substrate differs. One seam, two
/// backings (principle #8), rather than a second copy of chain-fill.
pub(super) enum DailyBacking {
    /// A `.org` directory is configured and the KB still ingests from it.
    Files(std::path::PathBuf),
    /// No dailies directory, or the KB's store is the source of truth. The daily
    /// is a node, addressed by `daily:YYYY-MM-DD`.
    Store,
}

impl Editor {
    /// Resolve the dailies directory. Explicit setting takes priority;
    /// falls back to `kb_notes_dir/daily`.
    pub fn kb_dailies_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref dir) = self.kb.dailies_dir {
            return Some(dir.clone());
        }
        self.kb.notes_dir.as_ref().map(|d| d.join("daily"))
    }

    /// Path for a daily note file: `dailies_dir/YYYY-MM-DD.org`.
    pub(super) fn kb_daily_path(&self, y: i32, m: u32, d: u32) -> Option<std::path::PathBuf> {
        self.kb_dailies_dir()
            .map(|dir| dir.join(format!("{}.org", mae_kb::activity::format_date(y, m, d))))
    }

    /// Canonical ID for a daily note.
    pub(super) fn kb_daily_id(y: i32, m: u32, d: u32) -> String {
        format!("daily:{}", mae_kb::activity::format_date(y, m, d))
    }

    /// Which backing serves dailies right now.
    ///
    /// `Store` whenever there is no dailies directory to write to, or the primary
    /// KB has been detached (`IngestPolicy::StoreIsTruth`) — writing a file that
    /// no ingest will ever read is worse than not writing one, because it looks
    /// like it worked.
    pub(super) fn kb_daily_backing(&self) -> DailyBacking {
        let store_is_truth = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.primary)
            .is_some_and(|i| !i.ingest_policy.allows_ingest());
        match self.kb_dailies_dir() {
            Some(dir) if !store_is_truth => DailyBacking::Files(dir),
            _ => DailyBacking::Store,
        }
    }

    /// Does this daily exist yet?
    pub(super) fn kb_daily_exists(&self, y: i32, m: u32, d: u32) -> bool {
        match self.kb_daily_backing() {
            DailyBacking::Files(_) => self
                .kb_daily_path(y, m, d)
                .map(|p| p.exists())
                .unwrap_or(false),
            DailyBacking::Store => {
                let id = Self::kb_daily_id(y, m, d);
                self.kb_get_node_anywhere(&id).is_some()
            }
        }
    }

    /// The daily's current text, whichever backing holds it.
    pub(super) fn kb_daily_text(&self, y: i32, m: u32, d: u32) -> Option<String> {
        match self.kb_daily_backing() {
            DailyBacking::Files(_) => {
                let path = self.kb_daily_path(y, m, d)?;
                std::fs::read_to_string(&path).ok()
            }
            DailyBacking::Store => self
                .kb_get_node_anywhere(&Self::kb_daily_id(y, m, d))
                .map(|n| n.body),
        }
    }

    /// Replace the daily's text.
    pub(super) fn kb_daily_set_text(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
        text: &str,
    ) -> Result<(), String> {
        match self.kb_daily_backing() {
            DailyBacking::Files(_) => {
                let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
                std::fs::write(&path, text).map_err(|e| format!("Failed to write daily: {e}"))?;
                self.kb.write_guard.insert(path.clone());
                self.kb_reimport_file(&path);
                self.kb.watcher_stats.reimports_total += 1;
                Ok(())
            }
            DailyBacking::Store => {
                let id = Self::kb_daily_id(y, m, d);
                // ADR-092's designated sole content mutator, deliberately --
                // a daily is a node, so it must not get its own private write
                // path. This is the write side of "one write path for a KB node".
                self.kb_update_node_with(&id, |n| n.body = text.to_string())
            }
        }
    }

    /// Open the daily for reading/editing.
    pub(super) fn kb_daily_open(&mut self, y: i32, m: u32, d: u32) -> Result<(), String> {
        match self.kb_daily_backing() {
            DailyBacking::Files(_) => {
                let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
                self.open_file_at_path(&path);
                Ok(())
            }
            DailyBacking::Store => {
                self.open_help_at(&Self::kb_daily_id(y, m, d));
                Ok(())
            }
        }
    }

    /// Ensure the daily exists, creating an empty one if not.
    ///
    /// Returns without doing anything when it already exists, so it is safe to
    /// call on every navigation.
    pub(super) fn kb_daily_ensure(&mut self, y: i32, m: u32, d: u32) -> Result<(), String> {
        if self.kb_daily_exists(y, m, d) {
            return Ok(());
        }
        let id = Self::kb_daily_id(y, m, d);
        let date_str = mae_kb::activity::format_date(y, m, d);
        match self.kb_daily_backing() {
            DailyBacking::Files(dir) => {
                if !dir.exists() {
                    std::fs::create_dir_all(&dir)
                        .map_err(|e| format!("Failed to create dailies dir: {e}"))?;
                }
                // The drawer is written because the FILE is the ingest source
                // here -- it is how the id reaches the store. On the store
                // backing the node carries its id directly, and #655 made the
                // drawer a rendering rather than a second home for it.
                let content = format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {date_str}\n\n");
                let path = dir.join(format!("{date_str}.org"));
                std::fs::write(&path, &content)
                    .map_err(|e| format!("Failed to write daily: {e}"))?;
                self.kb.write_guard.insert(path.clone());
                self.kb_reimport_file(&path);
                self.kb.watcher_stats.reimports_total += 1;
                Ok(())
            }
            DailyBacking::Store => {
                self.kb_create_node(&id, &date_str, "", mae_kb::NodeKind::Note)?;
                Ok(())
            }
        }
    }

    /// Find the nearest existing daily before/after a date.
    /// `direction`: -1 = backward, 1 = forward.
    pub(super) fn kb_daily_find_nearest(
        &self,
        y: i32,
        m: u32,
        d: u32,
        direction: i32,
    ) -> Option<(i32, u32, u32)> {
        let max_search = self.kb.daily_chain_gap_max;
        let step = if direction < 0 {
            mae_kb::activity::prev_day
        } else {
            mae_kb::activity::next_day
        };
        let mut cur = step(y, m, d);
        for _ in 0..max_search {
            if self.kb_daily_exists(cur.0, cur.1, cur.2) {
                return Some(cur);
            }
            cur = step(cur.0, cur.1, cur.2);
        }
        None
    }

    /// Chain-fill: ensure target date's daily exists and is linked back to
    /// the most recent pre-existing daily. Creates stub files for gaps.
    pub fn kb_daily_chain_fill(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
    ) -> Result<ChainFillResult, String> {
        let mut result = ChainFillResult {
            stubs_created: Vec::new(),
            links_inserted: 0,
            anchor_date: None,
        };

        // Ensure target date exists
        self.kb_daily_ensure(y, m, d)?;

        // Walk backwards to find the anchor (pre-existing daily)
        let max_gap = self.kb.daily_chain_gap_max;
        let mut cur = (y, m, d);
        let mut chain: Vec<(i32, u32, u32)> = vec![cur];

        for _ in 0..max_gap {
            let prev = mae_kb::activity::prev_day(cur.0, cur.1, cur.2);
            if self.kb_daily_exists(prev.0, prev.1, prev.2) {
                // This is a pre-existing daily — it's our anchor
                result.anchor_date = Some(prev);
                chain.push(prev);
                break;
            }
            // Create stub for the gap day
            self.kb_daily_ensure(prev.0, prev.1, prev.2)?;
            result.stubs_created.push(prev);
            chain.push(prev);
            cur = prev;
        }

        // Now insert "Previous:" links from newest → oldest
        // chain is [target, ..., anchor] so we link chain[i] → chain[i+1]
        for i in 0..chain.len().saturating_sub(1) {
            let (cy, cm, cd) = chain[i];
            let (py, pm, pd) = chain[i + 1];
            let prev_id = Self::kb_daily_id(py, pm, pd);
            let prev_date_str = mae_kb::activity::format_date(py, pm, pd);
            let link_line = format!("Previous: [[id:{}][{}]]", prev_id, prev_date_str);

            // Insert "Previous:" link on chain[i] pointing to chain[i+1]
            if self.kb_daily_insert_link(cy, cm, cd, "Previous:", &link_line)? {
                result.links_inserted += 1;
            }

            // Symmetric "Next:" link on chain[i+1] pointing back at chain[i].
            let next_id = Self::kb_daily_id(cy, cm, cd);
            let next_date_str = mae_kb::activity::format_date(cy, cm, cd);
            let next_link_line = format!("Next: [[id:{}][{}]]", next_id, next_date_str);
            if self.kb_daily_insert_link(py, pm, pd, "Next:", &next_link_line)? {
                result.links_inserted += 1;
            }
        }

        Ok(result)
    }

    /// Insert one chain link into a daily, once.
    ///
    /// The same algorithm the file backing always used -- find `#+title:`, insert
    /// after it, skip if a link of this kind is already present -- but expressed
    /// over `kb_daily_text`/`kb_daily_set_text` so it works on a node body too.
    /// It was written out twice (once for `Previous:`, once for `Next:`), which is
    /// how a store backing would have become four copies instead of one.
    ///
    /// Returns whether a link was actually inserted.
    fn kb_daily_insert_link(
        &mut self,
        y: i32,
        m: u32,
        d: u32,
        marker: &str,
        link_line: &str,
    ) -> Result<bool, String> {
        let Some(content) = self.kb_daily_text(y, m, d) else {
            return Ok(false);
        };
        if content.contains(marker) {
            return Ok(false);
        }
        let mut lines: Vec<&str> = content.lines().collect();
        let insert_pos = lines
            .iter()
            .position(|l| l.starts_with("#+title:"))
            .map(|i| i + 1)
            .unwrap_or(lines.len());
        lines.insert(insert_pos, link_line);
        let updated = lines.join("\n") + "\n";
        self.kb_daily_set_text(y, m, d, &updated)?;
        Ok(true)
    }

    /// Open today's daily with chain-fill.
    pub fn kb_goto_daily_today(&mut self) -> Result<(), String> {
        let (y, m, d) = today_ymd();
        self.kb_daily_chain_fill(y, m, d)?;
        self.kb_daily_open(y, m, d)
    }

    /// Open yesterday's daily.
    pub fn kb_goto_daily_yesterday(&mut self) -> Result<(), String> {
        let (y, m, d) = today_ymd();
        let (py, pm, pd) = mae_kb::activity::prev_day(y, m, d);
        if !self.kb_daily_exists(py, pm, pd) {
            self.kb_daily_ensure(py, pm, pd)?;
        }
        let path = self
            .kb_daily_path(py, pm, pd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Navigate to previous daily from current buffer's date.
    pub fn kb_daily_prev(&mut self) -> Result<(), String> {
        let (y, m, d) = self.kb_daily_date_from_buffer()?;
        let (py, pm, pd) = self
            .kb_daily_find_nearest(y, m, d, -1)
            .ok_or("No previous daily found")?;
        let path = self
            .kb_daily_path(py, pm, pd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Navigate to next daily from current buffer's date.
    pub fn kb_daily_next(&mut self) -> Result<(), String> {
        let (y, m, d) = self.kb_daily_date_from_buffer()?;
        let (ny, nm, nd) = self
            .kb_daily_find_nearest(y, m, d, 1)
            .ok_or("No next daily found")?;
        let path = self
            .kb_daily_path(ny, nm, nd)
            .ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Open a daily for a specific date string (YYYY-MM-DD).
    pub fn kb_goto_daily_date(&mut self, date_str: &str) -> Result<(), String> {
        let (y, m, d) = mae_kb::activity::parse_date(date_str)
            .ok_or_else(|| format!("Invalid date: '{}' (expected YYYY-MM-DD)", date_str))?;
        self.kb_daily_chain_fill(y, m, d)?;
        let path = self.kb_daily_path(y, m, d).ok_or("No dailies directory")?;
        self.open_file_at_path(&path);
        Ok(())
    }

    /// Extract a date from the current buffer's filename or title.
    pub(super) fn kb_daily_date_from_buffer(&self) -> Result<(i32, u32, u32), String> {
        let buf = &self.buffers[self.active_buffer_idx()];
        // Try filename: YYYY-MM-DD.org
        if let Some(fp) = buf.file_path() {
            if let Some(stem) = fp.file_stem().and_then(|s| s.to_str()) {
                if let Some(date) = mae_kb::activity::parse_date(stem) {
                    return Ok(date);
                }
            }
        }
        // Try title line: #+title: YYYY-MM-DD
        let content = buf.text();
        for line in content.lines().take(10) {
            if let Some(rest) = line.strip_prefix("#+title:") {
                let trimmed = rest.trim();
                if let Some(date) = mae_kb::activity::parse_date(trimmed) {
                    return Ok(date);
                }
            }
        }
        Err("Current buffer is not a daily note".to_string())
    }

    /// Open a file at a given path (helper for dailies navigation).
    pub(crate) fn open_file_at_path(&mut self, path: &std::path::Path) {
        // Check if buffer already open
        for (i, buf) in self.buffers.iter().enumerate() {
            if buf.file_path().map(|p| p == path).unwrap_or(false) {
                self.display_buffer(i);
                return;
            }
        }
        // Open new buffer
        match crate::buffer::Buffer::from_file(path) {
            Ok(mut buf) => {
                buf.name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("daily")
                    .to_string();
                self.buffers.push(buf);
                let idx = self.buffers.len() - 1;

                // Language detection (same as open_file_hidden in file_ops.rs)
                let detected_lang = self.buffers[idx]
                    .file_path()
                    .and_then(|p| crate::syntax::language_for_buffer(p, &self.buffers[idx].text()));
                if let Some(lang) = detected_lang {
                    self.syntax.set_language(idx, lang);
                    self.buffers[idx]
                        .local_options
                        .apply_defaults(&lang.default_local_options());
                }

                self.display_buffer(idx);
            }
            Err(e) => {
                self.set_status(format!("Failed to open daily: {}", e));
            }
        }
    }

    // --- Graph KB dispatch helpers (CozoDB backend) ---
}
