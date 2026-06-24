//! NotificationCenter — MAE's user-facing attention bus (ADR-024).
//!
//! Fills the gap between the transient, single-slot **status line** (clobbered by
//! any `set_status`) and the **modal `MiniDialog`** (blocking, was bespoke per
//! event). Background subsystems (collab, lsp, ai, save) raise a [`Notification`]
//! through `Editor::notify`; the center dedups by key, routes by severity to a
//! [`Surface`], and tracks *outstanding* sticky items so a non-clobberable
//! mode-line badge + a `*Notifications*` buffer can surface them.
//!
//! Precedent: Emacs `display-warning`/`*Warnings*` + `alert.el` severity routing;
//! VS Code `showXMessage(msg, ...actions)`; Neovim `vim.notify`/`vim.ui` pluggable
//! sinks. This module is the data model + routing only — the visual side effects
//! (status, badge, modal, buffer) live in `editor::notify_ops` because they need
//! `&mut Editor`.

use crate::messages::MessageLevel;
use std::collections::VecDeque;

/// How attention-worthy a notification is. `Ord` so a badge can pick the worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Success,
    Warning,
    Error,
    ActionRequired,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Success => "success",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::ActionRequired => "action-required",
        }
    }

    pub fn parse(s: &str) -> Option<Severity> {
        match s {
            "info" => Some(Severity::Info),
            "success" => Some(Severity::Success),
            "warning" => Some(Severity::Warning),
            "error" => Some(Severity::Error),
            "action-required" | "action_required" => Some(Severity::ActionRequired),
            _ => None,
        }
    }

    /// Mapping into the `*Messages*` log level (the bus mirrors there for parity).
    pub fn message_level(self) -> MessageLevel {
        match self {
            Severity::Info | Severity::Success => MessageLevel::Info,
            Severity::Warning | Severity::ActionRequired => MessageLevel::Warn,
            Severity::Error => MessageLevel::Error,
        }
    }
}

/// Where a notification is surfaced. The routing policy maps severity → surface
/// (overridable per-severity via the `notify_route_*` options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// Transient status line (reuses `set_status`). No sticky entry, no badge.
    Status,
    /// Sticky: counts in the mode-line attention badge + lists in `*Notifications*`.
    Badge,
    /// Blocking modal mini-dialog (for `Lifetime::BlockingReply`).
    Modal,
    /// Sticky in `*Notifications*` (+ badge); the default for action-required.
    Buffer,
    /// Recorded in the feed/log only — no visible surface.
    Silent,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Status => "status",
            Surface::Badge => "badge",
            Surface::Modal => "modal",
            Surface::Buffer => "buffer",
            Surface::Silent => "silent",
        }
    }

    pub fn parse(s: &str) -> Option<Surface> {
        match s {
            "status" => Some(Surface::Status),
            "badge" => Some(Surface::Badge),
            "modal" => Some(Surface::Modal),
            "buffer" => Some(Surface::Buffer),
            "silent" => Some(Surface::Silent),
            _ => None,
        }
    }

    /// Does this surface keep the item as an *outstanding* sticky entry (badge +
    /// buffer), versus a fire-and-forget transient/silent surface?
    pub fn is_sticky(self) -> bool {
        matches!(self, Surface::Badge | Surface::Buffer | Surface::Modal)
    }
}

/// What an at-point action invokes. We reuse the command-dispatch spine (named
/// commands / structured collab verbs) rather than closures — special buffers
/// re-dispatch by name, and closures aren't `Clone`/`Send`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifCommand {
    /// Run a registered editor command by name (e.g. "view-messages").
    Command(String),
    /// Collab: adopt the daemon's authoritative node (drop local divergence).
    AdoptRemote { kb_id: String, node_id: String },
    /// Collab: adopt authoritative, then re-author the local content under the
    /// current epoch (the graceful keep-mine path).
    KeepMine { kb_id: String, node_id: String },
    /// Collab: export the diverged node externally, then adopt authoritative.
    StashExternally { kb_id: String, node_id: String },
    /// Answer a `BlockingReply` notification (e.g. the host-key TOFU prompt) over
    /// the bus by sending this boolean on its reply channel — so headless/MCP
    /// (`notify_resolve`) and the `*Notifications*` row can answer a modal without
    /// a GUI keypress (B-22c). For a `Text` reply, `true`→"y", `false`→"".
    Reply(bool),
}

/// A labelled at-point action on a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationAction {
    pub label: String,
    pub command: NotifCommand,
}

/// How long a notification persists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    /// Status-line only, no sticky entry.
    Transient,
    /// Stays until resolved/dismissed; counts in the badge + `*Notifications*`.
    Sticky,
    /// Modal; the raiser blocks on a reply channel (see [`NotifReply`]).
    BlockingReply,
}

/// How a sticky notification ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Dismissed,
    Acted(String),
    Replied,
}

/// Reply channel for a `BlockingReply` notification — generalizes the bespoke
/// `pending_host_key_reply`. Kept as `std::sync::mpsc` to match the existing
/// `CollabEvent::HostKeyPrompt{reply}` type (zero daemon-side change).
pub enum NotifReply {
    Bool(std::sync::mpsc::Sender<bool>),
    Text(std::sync::mpsc::Sender<String>),
}

/// A stored notification.
#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u64,
    /// Dedup identity — re-raising the same key updates one item (no spam).
    pub key: Option<String>,
    pub severity: Severity,
    pub source: &'static str,
    pub title: String,
    pub body: Option<String>,
    pub actions: Vec<NotificationAction>,
    pub lifetime: Lifetime,
    /// Monotonic ordering (bumped on dedup update so it sorts as freshest).
    pub created_seq: u64,
    /// `Some` once resolved (kept briefly in `active` as a "recently-resolved" row).
    pub resolved: Option<Resolution>,
}

impl Notification {
    /// Start building an Info notification. `editor.notify(...)` consumes the builder.
    pub fn info(source: &'static str, title: impl Into<String>) -> NotificationBuilder {
        NotificationBuilder::new(Severity::Info, source, title)
    }
    pub fn success(source: &'static str, title: impl Into<String>) -> NotificationBuilder {
        NotificationBuilder::new(Severity::Success, source, title)
    }
    pub fn warning(source: &'static str, title: impl Into<String>) -> NotificationBuilder {
        NotificationBuilder::new(Severity::Warning, source, title)
    }
    pub fn error(source: &'static str, title: impl Into<String>) -> NotificationBuilder {
        NotificationBuilder::new(Severity::Error, source, title)
    }
    pub fn action_required(source: &'static str, title: impl Into<String>) -> NotificationBuilder {
        NotificationBuilder::new(Severity::ActionRequired, source, title)
    }
}

/// Builder for a notification (VS Code-style ergonomics). Carries an optional
/// reply channel that `Editor::notify` extracts into the modal slot.
pub struct NotificationBuilder {
    pub key: Option<String>,
    pub severity: Severity,
    pub source: &'static str,
    pub title: String,
    pub body: Option<String>,
    pub actions: Vec<NotificationAction>,
    pub lifetime: Lifetime,
    pub reply: Option<NotifReply>,
}

impl NotificationBuilder {
    fn new(severity: Severity, source: &'static str, title: impl Into<String>) -> Self {
        NotificationBuilder {
            key: None,
            severity,
            source,
            title: title.into(),
            body: None,
            actions: Vec::new(),
            // Action-required defaults to sticky; everything else transient.
            lifetime: if severity == Severity::ActionRequired {
                Lifetime::Sticky
            } else {
                Lifetime::Transient
            },
            reply: None,
        }
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }
    pub fn action(mut self, label: impl Into<String>, command: NotifCommand) -> Self {
        self.actions.push(NotificationAction {
            label: label.into(),
            command,
        });
        self
    }
    pub fn lifetime(mut self, lifetime: Lifetime) -> Self {
        self.lifetime = lifetime;
        self
    }
    /// Make this a blocking prompt with the given reply channel.
    pub fn blocking(mut self, reply: NotifReply) -> Self {
        self.lifetime = Lifetime::BlockingReply;
        self.reply = Some(reply);
        self
    }
}

/// Routing decision returned by [`NotificationCenter::ingest`] so the Editor can
/// apply the matching visual side effect.
pub struct Ingested {
    pub id: u64,
    pub surface: Surface,
    /// True when this replaced an existing item with the same key (dedup update).
    pub deduped: bool,
}

/// The attention bus. Main-thread, single-owner (like `mini_dialog`); collab/
/// lsp/ai events already arrive on the main loop tick, so no `Arc<Mutex>`.
pub struct NotificationCenter {
    active: Vec<Notification>,
    feed: VecDeque<Notification>,
    next_id: u64,
    next_seq: u64,
    max_feed: usize,
    max_resolved_kept: usize,
    // Routing policy (OptionRegistry-backed via editor::option_ops match arms).
    pub route_info: Surface,
    pub route_success: Surface,
    pub route_warning: Surface,
    pub route_error: Surface,
    pub route_action_required: Surface,
    pub badge_min_severity: Severity,
}

impl Default for NotificationCenter {
    fn default() -> Self {
        NotificationCenter {
            active: Vec::new(),
            feed: VecDeque::new(),
            next_id: 1,
            next_seq: 0,
            max_feed: 500,
            max_resolved_kept: 50,
            // alert.el-style defaults: chatty info on the status line; warnings/
            // errors quietly in the badge; action-required in the buffer.
            route_info: Surface::Status,
            route_success: Surface::Status,
            route_warning: Surface::Badge,
            route_error: Surface::Badge,
            route_action_required: Surface::Buffer,
            badge_min_severity: Severity::Warning,
        }
    }
}

impl NotificationCenter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The configured surface for a severity (before the `BlockingReply` override).
    pub fn route_for(&self, severity: Severity) -> Surface {
        match severity {
            Severity::Info => self.route_info,
            Severity::Success => self.route_success,
            Severity::Warning => self.route_warning,
            Severity::Error => self.route_error,
            Severity::ActionRequired => self.route_action_required,
        }
    }

    /// Effective surface, honoring the `BlockingReply` → `Modal` escalation.
    pub fn surface_for(&self, severity: Severity, lifetime: Lifetime) -> Surface {
        if lifetime == Lifetime::BlockingReply {
            Surface::Modal
        } else {
            self.route_for(severity)
        }
    }

    /// Add (or dedup-update) a notification and return the routing decision.
    pub fn ingest(&mut self, b: &NotificationBuilder) -> Ingested {
        let surface = self.surface_for(b.severity, b.lifetime);
        let seq = self.next_seq;
        self.next_seq += 1;

        // Dedup: an unresolved active item with the same key is updated in place.
        let existing = b.key.as_ref().and_then(|k| {
            self.active
                .iter_mut()
                .find(|n| n.resolved.is_none() && n.key.as_deref() == Some(k.as_str()))
        });

        let id = if let Some(n) = existing {
            n.severity = b.severity;
            n.title = b.title.clone();
            n.body = b.body.clone();
            n.actions = b.actions.clone();
            n.lifetime = b.lifetime;
            n.source = b.source;
            n.created_seq = seq;
            n.id
        } else {
            let id = self.next_id;
            self.next_id += 1;
            let n = Notification {
                id,
                key: b.key.clone(),
                severity: b.severity,
                source: b.source,
                title: b.title.clone(),
                body: b.body.clone(),
                actions: b.actions.clone(),
                lifetime: b.lifetime,
                created_seq: seq,
                resolved: None,
            };
            // Only sticky surfaces retain an active entry; transient/silent just feed.
            if surface.is_sticky() {
                self.active.push(n.clone());
            }
            self.push_feed(n);
            return Ingested {
                id,
                surface,
                deduped: false,
            };
        };

        // Dedup path: also record the update in the feed.
        if let Some(n) = self.active.iter().find(|n| n.id == id) {
            self.push_feed(n.clone());
        }
        Ingested {
            id,
            surface,
            deduped: true,
        }
    }

    fn push_feed(&mut self, n: Notification) {
        if self.feed.len() >= self.max_feed {
            self.feed.pop_front();
        }
        self.feed.push_back(n);
    }

    pub fn get(&self, id: u64) -> Option<&Notification> {
        self.active.iter().find(|n| n.id == id)
    }

    pub fn action(&self, id: u64, idx: usize) -> Option<&NotificationAction> {
        self.get(id).and_then(|n| n.actions.get(idx))
    }

    /// Mark a notification resolved (kept briefly as a recently-resolved row).
    pub fn resolve(&mut self, id: u64, resolution: Resolution) -> bool {
        let found = if let Some(n) = self.active.iter_mut().find(|n| n.id == id) {
            n.resolved = Some(resolution);
            true
        } else {
            false
        };
        if found {
            self.prune_resolved();
        }
        found
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        self.resolve(id, Resolution::Dismissed)
    }

    /// Keep at most `max_resolved_kept` recently-resolved rows (drop the oldest).
    fn prune_resolved(&mut self) {
        let resolved: Vec<u64> = self
            .active
            .iter()
            .filter(|n| n.resolved.is_some())
            .map(|n| n.id)
            .collect();
        if resolved.len() > self.max_resolved_kept {
            let drop_n = resolved.len() - self.max_resolved_kept;
            let to_drop: std::collections::HashSet<u64> =
                resolved.into_iter().take(drop_n).collect();
            self.active.retain(|n| !to_drop.contains(&n.id));
        }
    }

    /// Outstanding = unresolved sticky items at/above the badge threshold.
    pub fn outstanding_count(&self) -> usize {
        self.active
            .iter()
            .filter(|n| n.resolved.is_none() && n.severity >= self.badge_min_severity)
            .count()
    }

    /// Worst severity among outstanding items (drives the badge glyph/color).
    pub fn badge_severity(&self) -> Option<Severity> {
        self.active
            .iter()
            .filter(|n| n.resolved.is_none() && n.severity >= self.badge_min_severity)
            .map(|n| n.severity)
            .max()
    }

    /// Active rows (outstanding first, then recently-resolved), freshest within
    /// each group — for the `*Notifications*` buffer.
    pub fn active_sorted(&self) -> Vec<&Notification> {
        let mut out: Vec<&Notification> = self.active.iter().collect();
        out.sort_by(|a, b| {
            a.resolved
                .is_some()
                .cmp(&b.resolved.is_some())
                .then(b.created_seq.cmp(&a.created_seq))
        });
        out
    }

    pub fn feed(&self) -> impl Iterator<Item = &Notification> {
        self.feed.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_by_severity_and_blocking_override() {
        let c = NotificationCenter::new();
        assert_eq!(c.route_for(Severity::Info), Surface::Status);
        assert_eq!(c.route_for(Severity::Warning), Surface::Badge);
        assert_eq!(c.route_for(Severity::ActionRequired), Surface::Buffer);
        // BlockingReply always escalates to Modal regardless of severity routing.
        assert_eq!(
            c.surface_for(Severity::Info, Lifetime::BlockingReply),
            Surface::Modal
        );
    }

    #[test]
    fn dedup_by_key_updates_one_item_no_spam() {
        let mut c = NotificationCenter::new();
        let i1 = c.ingest(&Notification::action_required("collab", "fenced once").key("k1"));
        let i2 = c.ingest(&Notification::action_required("collab", "fenced again").key("k1"));
        assert!(!i1.deduped);
        assert!(i2.deduped);
        assert_eq!(i1.id, i2.id, "same key reuses the same notification id");
        assert_eq!(c.outstanding_count(), 1, "no spam — one outstanding item");
        assert_eq!(
            c.get(i1.id).unwrap().title,
            "fenced again",
            "updated in place"
        );
    }

    #[test]
    fn transient_info_does_not_become_outstanding() {
        let mut c = NotificationCenter::new();
        c.ingest(&Notification::info("collab", "connected"));
        assert_eq!(c.outstanding_count(), 0, "Status-routed info is not sticky");
        // ...but it is recorded in the feed.
        assert_eq!(c.feed().count(), 1);
    }

    #[test]
    fn resolve_stops_counting_and_picks_worst_severity() {
        let mut c = NotificationCenter::new();
        let warn = c.ingest(&Notification::warning("save", "conflict").key("w"));
        let act = c.ingest(&Notification::action_required("collab", "fenced").key("a"));
        assert_eq!(c.outstanding_count(), 2);
        assert_eq!(c.badge_severity(), Some(Severity::ActionRequired));
        assert!(c.resolve(act.id, Resolution::Acted("Accept-remote".into())));
        assert_eq!(c.outstanding_count(), 1, "resolved item stops counting");
        assert_eq!(c.badge_severity(), Some(Severity::Warning));
        assert!(c.dismiss(warn.id));
        assert_eq!(c.outstanding_count(), 0);
        assert_eq!(c.badge_severity(), None);
    }

    #[test]
    fn badge_threshold_filters_low_severity() {
        let mut c = NotificationCenter::new();
        // Success routed to Status isn't sticky anyway; force a sticky low-sev item
        // by routing success to the badge, then confirm the threshold filters it.
        c.route_success = Surface::Badge;
        c.ingest(&Notification::success("test", "done").key("s"));
        assert_eq!(
            c.outstanding_count(),
            0,
            "below badge_min_severity (Warning) — not counted"
        );
    }

    #[test]
    fn feed_is_bounded() {
        let mut c = NotificationCenter::new();
        c.max_feed = 3;
        for i in 0..5 {
            c.ingest(&Notification::info("t", format!("m{i}")));
        }
        assert_eq!(c.feed().count(), 3, "feed evicts oldest");
    }
}
