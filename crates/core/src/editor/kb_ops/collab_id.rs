//! The editor's KB collab-id resolution (ADR-105 D4).
//!
//! Split from `kb_ops/sync.rs` (structural ceiling) and kept together because
//! these two functions answer the same question from opposite ends: what id does
//! this KB sync under, and what id did the human mean. Both exist because a KB's
//! display NAME and its collab ID stopped being the same string in D4 — see
//! `mae_kb::kb_identity` for why that had to change.

use super::*;

impl Editor {
    /// Resolve a KB argument a human typed into the collab id to put on the wire
    /// (ADR-105 D4).
    ///
    /// Every `:kb-…` command takes one string from the user, and after D4 that
    /// string can be two different things: the KB's **display name** (what a person
    /// remembers and what `:kb-share collabtest` accepts) or its **collab id**
    /// (what actually addresses it, and what a peer hands you out of band). Before
    /// D4 those were the same string, so passing the argument through untouched was
    /// correct. It stopped being correct the moment ids were minted, and it fails in
    /// the least helpful way available: `:kb-approve collabtest …` reaches the
    /// daemon as a KB that does not exist, so approval silently applies to nothing.
    ///
    /// Resolution order is id-first so an explicit id always wins over a name that
    /// happens to collide with one. An argument matching NEITHER is passed through
    /// unchanged — that is the joiner's case, where a peer's id names a KB this
    /// editor has never seen and could not possibly resolve.
    pub fn kb_collab_id_arg(&self, arg: &str) -> String {
        if self.kb.registry.target_of_collab_id(arg).is_some() {
            return arg.to_string();
        }
        self.kb
            .registry
            .target_of_name(arg)
            .and_then(|t| self.kb.registry.collab_id_of_target(&t))
            .unwrap_or_else(|| arg.to_string())
    }

    pub fn kb_collab_id_for_share(&mut self, target: &mae_kb::KbTarget) -> Option<String> {
        match self.mae_data_dir() {
            Some(dir) => {
                let (registry, id, saved) = mae_kb::federation::KbRegistry::update(&dir, |reg| {
                    reg.collab_id_for_share(target)
                });
                if let Err(e) = saved {
                    // An unpersisted mint yields a DIFFERENT id next time, orphaning
                    // this share's collection and membership on the daemon.
                    tracing::error!(error = %e, "failed to persist KB collab id");
                    return None;
                }
                self.kb.registry = registry;
                self.kb.last_local_registry_write = Some(std::time::Instant::now());
                Some(id)
            }
            // No data dir (headless/test): mint in memory so the session still
            // works. Nothing durable is written, so nothing can be orphaned.
            None => Some(self.kb.registry.collab_id_for_share(target)),
        }
    }
}
