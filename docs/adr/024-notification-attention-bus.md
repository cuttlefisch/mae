# ADR-024: Notification / attention bus

**Status:** Accepted. Core + routing implemented; the mode-line badge, the
`*Notifications*` buffer, the generalized modal (TOFU migration), and the collab
consumer land in subsequent phases (see the plan).
**Relates to:** ADR-017 (TOFU host-key prompt — the first blocking consumer),
ADR-020 (the magit-style `*KB Sharing*` admin buffer reuses the same assembly),
ADR-023 (the collab epoch fence — the motivating action-required event).

## Context

Background subsystems (collab daemon events, LSP, AI, file watchers, save
coordination) need to raise events that **require the user's attention or
action**, at any time. Before this ADR, MAE had only three surfaces and no bus:

1. **Status line** (`Editor::set_status` → single-slot `status_msg: String`):
   transient and **clobbered** by any other subsystem calling `set_status` (a
   live collab test confirmed an important "your edit didn't sync" notice was
   drowned out by spinner updates).
2. **`*Messages*` buffer** (`MessageLog` ring buffer; `tracing::warn!` lands
   here): a passive developer log — action-required items get buried.
3. **Modal `MiniDialog`** (the TOFU host-key prompt): blocks everything, and was
   **bespoke per event** — a one-off `pending_host_key_reply` field + ad-hoc
   surfacing. Every new attention case would hand-roll its own.

**The gap:** there is no durable, non-clobberable channel for *outstanding*
action-required items between "transient status" and "hard modal", and no general
bus. This is the human-facing half of ADR-023: a member whose edit is fenced sees
their local copy showing the edit but it never reaches peers — and the only signal
is a buried log line.

## Precedent

- **Emacs**: `display-warning` + the `*Warnings*` buffer with
  `warning-minimum-level` thresholds; `message` (transient) vs `*Messages*`
  (log); `y-or-n-p`/`yes-or-no-p` (blocking); mode-line "lighters" (persistent
  indicators); and `alert.el` — severity → routed "style" (message / mode-line /
  libnotify / buffer) with per-category rules.
- **VS Code**: `showInformationMessage/showWarningMessage/showErrorMessage(msg,
  ...actions)` returning the chosen action; non-modal toasts vs `{modal:true}`;
  status-bar items + a notification-count indicator.
- **Neovim**: `vim.notify(msg, level)` as a *pluggable* sink and
  `vim.ui.select`/`vim.ui.input` as *pluggable* prompt providers — the same call
  works across TUI / GUI / headless.

## Decision

Add a **`NotificationCenter`** on `Editor` (main-thread, single-owner — like
`mini_dialog`; collab/LSP/AI events already arrive on the main loop tick, so no
`Arc<Mutex>`). Each notification mirrors into the thread-safe `MessageLog` for
parity (so `read_messages`/AI see the feed). Surfaces are **pluggable sinks**;
the bus is the single choke-point.

**Notification** (`crates/core/src/notifications.rs`): `severity`
(Info/Success/Warning/Error/ActionRequired, `Ord`), `source` (collab/lsp/ai/…),
`title`, `body`, `actions` (a label + a `NotifCommand` — a **named command or
structured verb**, not a closure, since special buffers re-dispatch by name and
closures aren't `Clone`/`Send`), a **dedup `key`** (re-raising updates one item,
no spam), `lifetime` (Transient/Sticky/BlockingReply), and an optional
`NotifReply` channel that generalizes `pending_host_key_reply` (kept as
`std::sync::mpsc` to match `CollabEvent::HostKeyPrompt{reply}` — zero daemon-side
change).

**Routing → surfaces**, severity-keyed and configurable via the OptionRegistry
(principle #7 — `notify_route_{info,success,warning,error,action_required}` +
`notify_badge_min_severity`, each a typed field + a `get_option`/`set_option`
match arm, Scheme-accessible). Defaults (alert.el-style): Info/Success →
**Status**; Warning/Error → **Badge**; ActionRequired → **Buffer** (+Badge);
`Lifetime::BlockingReply` always escalates to **Modal**. Error also rings the
existing visual bell.

The five surfaces:
- **(a) Transient status** — reuse `set_status` for chatty Info/Success.
- **(b) Mode-line attention badge** — a NEW segment in `build_status_segments`, a
  **pure fn of `&Editor`** reading `outstanding_count()`/`badge_severity()`.
  **Non-clobberable by construction** (reads the center, never `status_msg`).
- **(c) Generalized modal** — one `pending_notif_reply: Option<(id, NotifReply)>`
  slot replaces the bespoke `pending_host_key_reply`; a `BlockingReply` routes to
  a `MiniDialog` via `MiniDialogContext::Notification`. TOFU becomes one consumer
  of this generic mechanism — future blocking prompts add no new fields.
- **(d) `*Notifications*` buffer** — a magit-style list of outstanding +
  recently-resolved items, foldable by category, with at-point actions
  (run-action / dismiss). Built on the git-status assembly (BufferKind +
  view-model + `render_common` spans + buffer-local keymap + cursor-context
  dispatch). The ADR-020 `*KB Sharing*` admin console reuses the same assembly.
- **(e) Silent** — feed/log only.

**Headless / MCP parity:** a `BlockingReply` with no human routes through the
existing `PendingInteractiveEvent`/`ask_user` machinery (the AI/agent answers it);
read-only `notifications_list` + `notify_resolve{id, action}` MCP/Scheme queries
let an agent enumerate + act on outstanding items (the Neovim pluggable-provider
analogy).

## Why not

- **Keep clobberable `set_status` for everything** — the demonstrated failure
  mode; attention-worthy events get lost. Rejected.
- **A toast/growl popup as the primary surface** — transient popups are missable
  too; the *durable* signal must be the non-clobberable badge + a buffer the user
  can return to. A desktop-notification (libnotify) sink is a future *additional*
  pluggable surface, not the backbone.
- **Per-event bespoke modals** (status quo) — doesn't scale; every case re-hand-
  rolls a reply field. The generalized reply slot subsumes them.

## Consequences

- Attention-required events have a reliable home: a non-clobberable badge + a
  resolvable buffer + (only when warranted) a modal — configurable per severity.
- The TOFU host-key prompt is re-expressed as one consumer of the bus; the
  bespoke field is deleted.
- The collab-divergence lifecycle (fenced/role-denied/reconnect-would-overwrite)
  becomes the first real consumer, with at-point Accept-remote / Keep-mine
  (re-author) / Stash-externally actions backed by an adopt-and-re-author
  primitive — closing the "granted editor is stuck" gap from the ADR-023 live run.
- **Follow-up (tracked):** sweep MAE's existing broadcasters / interaction points
  (LSP diagnostics, AI proposals, save conflicts, file-watcher external-change,
  build/test results) and migrate the *appropriate* ones into consumers — pure
  transient info stays on `set_status`.

## Verification

Unit: dedup-by-key updates one item; severity→surface routing honors options;
`outstanding_count`/`badge_severity` correct across notify/resolve/dismiss;
resolved items stop counting; feed bounded. Integration: raise a notification at
each severity and observe status / badge / `*Notifications*`; run an action via
MCP `notify_resolve`; confirm the TOFU prompt still works (same y/N); drive the
collab fenced-edit end-to-end (Keep-mine converges).
