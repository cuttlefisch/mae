//! A read or write that NAMES a KB acts on that KB or fails.
//!
//! The one place that decides which KB a node-addressed operation means:
//! `kb_write_owner` (writes, in `registry.rs`) and the read/promote/restore
//! paths below all ask [`Editor::kb_refuse_ambiguous_id`] and
//! [`Editor::kb_resolve_target`], so an id held by several KBs is never
//! answered from whichever one a hash map yielded first.

use mae_kb::graph_query::ScopedBackend;
use mae_kb::KbStore;

use crate::editor::Editor;

/// Split a leading `--kb=NAME` off an ex-command argument string: the KB a
/// `:kb-create` / `:kb-update` / `:kb-delete` / `:kb-set-role` names, and the
/// rest. A KB name containing whitespace cannot be spelled this way.
pub(crate) fn take_kb_flag(input: &str) -> (Option<&str>, &str) {
    let input = input.trim();
    match input.strip_prefix("--kb=") {
        Some(rest) => {
            let (name, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            (Some(name), tail.trim_start())
        }
        None => (None, input),
    }
}

impl Editor {
    /// `:kb-create [--kb=NAME] <id> <title>` or `:kb-create <title>`. Naming a KB
    /// needs an explicit id: the title-only form is the capture flow, which
    /// places the note by `kb_notes_dir`, not by a named KB.
    pub(crate) fn ex_kb_create(&mut self, input: &str) {
        let (kb, rest) = take_kb_flag(input);
        if rest.is_empty() {
            self.set_status("Usage: :kb-create [--kb=NAME] <id> <title>  or  :kb-create <title>");
            return;
        }
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let explicit_id = parts.len() == 2 && (parts[0].contains(':') || !parts[0].contains(' '));
        let result = match (explicit_id, kb) {
            (true, _) => self.kb_create_node_target(
                kb,
                parts[0],
                parts[1].trim(),
                "",
                mae_kb::NodeKind::Note,
            ),
            (false, None) => self.kb_create_note_from_title(rest).map(|_| ()),
            (false, Some(_)) => {
                Err("--kb needs an explicit id: :kb-create --kb=NAME <id> <title>".into())
            }
        };
        if let Err(e) = result {
            self.set_status(e);
        }
    }

    /// `:kb-update [--kb=NAME] <id> <body…>`.
    pub(crate) fn ex_kb_update(&mut self, input: &str) {
        let (kb, rest) = take_kb_flag(input);
        if rest.is_empty() {
            self.set_status("Usage: :kb-update [--kb=NAME] <id> <body>");
            return;
        }
        let (id, body) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if let Err(e) = self.kb_update_node_target(kb, id.trim(), None, Some(body.trim()), None) {
            self.set_status(e);
        }
    }

    /// `:kb-delete [--kb=NAME] <id>`.
    pub(crate) fn ex_kb_delete(&mut self, input: &str) {
        let (kb, id) = take_kb_flag(input);
        if id.is_empty() {
            self.set_status("Usage: :kb-delete [--kb=NAME] <id>");
            return;
        }
        if let Err(e) = self.kb_delete_node_target(kb, id) {
            self.set_status(e);
        }
    }

    /// `:kb-set-role [--kb=NAME] <id> <role>`.
    pub(crate) fn ex_kb_set_role(&mut self, input: &str) {
        let (kb, rest) = take_kb_flag(input);
        let mut parts = rest.split_whitespace();
        let (id, role) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        if id.is_empty() || role.is_empty() {
            self.set_status(
                "usage: :kb-set-role [--kb=NAME] <node-id> <source|atom|molecule|hub>".to_string(),
            );
            return;
        }
        match self.kb_set_role_target(kb, id, role) {
            Ok(msg) => self.set_status(msg),
            Err(e) => self.set_status(e),
        }
    }

    /// Names of every KB holding `id`: each loaded instance, plus `local` when
    /// the primary holds it as its own (a node the primary merely carries for a
    /// joined KB is that KB's, not `local`'s). Sorted.
    pub(crate) fn kb_id_holders(&self, id: &str) -> Vec<String> {
        let mut holders: Vec<String> = self
            .kb
            .instances
            .iter()
            .filter(|(_, kb)| kb.contains(id))
            .map(|(uuid, _)| {
                self.kb
                    .registry
                    .find_by_uuid(uuid)
                    .map_or_else(|| uuid.clone(), |e| e.name.clone())
            })
            .collect();
        let primary_holds = self
            .kb
            .primary
            .get(id)
            .is_some_and(|n| n.source != Some(mae_kb::NodeSource::Federation));
        if primary_holds {
            holders.push("local".to_string());
        }
        holders.sort();
        holders
    }

    /// Refuse an unqualified `id` that more than one KB holds, naming them.
    /// `verb` completes "name the one to …".
    pub(crate) fn kb_refuse_ambiguous_id(&self, id: &str, verb: &str) -> Result<(), String> {
        let holders = self.kb_id_holders(id);
        if holders.len() > 1 {
            return Err(format!(
                "'{id}' exists in several KBs ({}); name the one to {verb}",
                holders.join(", ")
            ));
        }
        Ok(())
    }

    /// The KB a read of `id` means. `Ok(None)`: no KB named and `id` is
    /// unambiguous, so the caller resolves as it always did. `Ok(Some(owner))`:
    /// the named KB (`None` inside = the primary), which must hold `id`.
    pub fn kb_read_scope(
        &self,
        id: &str,
        kb: Option<&str>,
    ) -> Result<Option<Option<String>>, String> {
        let Some(name) = kb else {
            self.kb_refuse_ambiguous_id(id, "read")?;
            return Ok(None);
        };
        let owner = self.kb_resolve_target(name)?;
        let backend = self.kb_scoped_backend(&owner);
        if !backend.holds(id) {
            return Err(format!("No KB node '{id}' in KB '{name}'"));
        }
        Ok(Some(owner))
    }

    /// The graph backend over exactly the KB `owner` names (`None` = primary).
    /// Prefers its durable store, else its in-memory copy.
    pub fn kb_scoped_backend(&self, owner: &Option<String>) -> ScopedBackend<'_> {
        match owner {
            None => match self.kb.store.as_deref() {
                Some(s) => ScopedBackend::Store(s, None),
                None => ScopedBackend::Mem(&self.kb.primary, None),
            },
            Some(uuid) => {
                let name = self
                    .kb
                    .registry
                    .find_by_uuid(uuid)
                    .map(|e| e.name.clone())
                    .or_else(|| Some(uuid.clone()));
                match self.kb.instance_stores.get(uuid) {
                    Some(s) => ScopedBackend::Store(&**s, name),
                    // `kb_resolve_target` verified the instance is loaded.
                    None => ScopedBackend::Mem(&self.kb.instances[uuid], name),
                }
            }
        }
    }

    /// The durable store of the KB named `kb` (`local` = the primary). An
    /// unknown, unloaded or store-less KB is an error; never another KB's store.
    pub fn kb_store_named(&self, kb: &str) -> Result<&dyn KbStore, String> {
        match self.kb_resolve_target(kb)? {
            None => self
                .kb
                .store
                .as_deref()
                .ok_or_else(|| "No KB store configured".to_string()),
            Some(uuid) => self
                .kb
                .instance_stores
                .get(&uuid)
                .map(|s| &**s as &dyn KbStore)
                .ok_or_else(|| format!("KB '{kb}' has no durable store")),
        }
    }
}
