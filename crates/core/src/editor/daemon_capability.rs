//! Daemon capability model (ADR-035): the single source of truth for *which
//! features need a daemon* and *whether they're available right now*.
//!
//! Replaces the ad-hoc per-call guard strings (e.g. the bespoke "not connected to
//! a daemon — start one with …" messages on the P2P paths) with one model that
//! every surface — the human (editor commands + buffers), the AI peer (MCP), and
//! Scheme — queries identically (principles #3 + #7). A feature answers, *before*
//! the action runs, with an actionable reason rather than a failed call.
//!
//! Three tiers (ADR-035 §"capability model"):
//! - **None** — must always work; the in-process floor (local KB, agenda, search,
//!   LSP/DAP, AI, editing). A daemon is never required.
//! - **Recommends** — works degraded without a daemon, better with one (KB
//!   hosting / thin-client, cross-session persistence, multi-frontend sharing).
//! - **Requires** — cannot work without a daemon (P2P KB sharing; continuous
//!   shared-KB sync that prevents offline-edit divergence; mesh membership).

use super::DaemonMode;

/// How much a feature depends on a daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonRequirement {
    /// Always works in-process; a daemon is never needed.
    None,
    /// Works without a daemon (possibly degraded); a daemon improves it.
    Recommends,
    /// Cannot function without a daemon.
    Requires,
}

impl DaemonRequirement {
    /// Stable wire string for introspection (Scheme/AI parity).
    pub fn as_str(&self) -> &'static str {
        match self {
            DaemonRequirement::None => "none",
            DaemonRequirement::Recommends => "recommends",
            DaemonRequirement::Requires => "requires",
        }
    }
}

/// The runtime daemon state a feature's availability is evaluated against. Built
/// from the editor's daemon/collab state ([`super::Editor::daemon_state_snapshot`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonStateSnapshot {
    /// Configured editor↔daemon relationship.
    pub mode: DaemonMode,
    /// The P2P control channel is wired (daemon reachable for control ops).
    pub control_wired: bool,
    /// The daemon read layer (LRU) is active.
    pub read_layer_up: bool,
    /// A collab session is live (the daemon is actively syncing shared state).
    pub collab_connected: bool,
    /// The daemon hosts the primary KB right now.
    pub hosting_primary: bool,
}

impl DaemonStateSnapshot {
    /// Is *any* daemon presence established (control or read layer)?
    pub fn daemon_present(&self) -> bool {
        self.control_wired || self.read_layer_up
    }
}

/// The answer to "can I use this feature right now?" — carries the *why* so a
/// surface can show an actionable reason instead of a silent failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureAvailability {
    /// Fully available.
    Available,
    /// Works, but in a reduced form without a daemon.
    Degraded { reason: String },
    /// Cannot run; `fix` says how to make it available.
    Unavailable { reason: String, fix: String },
}

impl FeatureAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, FeatureAvailability::Available)
    }
    /// A short status word for compact surfaces (mode-line, tables).
    pub fn label(&self) -> &'static str {
        match self {
            FeatureAvailability::Available => "available",
            FeatureAvailability::Degraded { .. } => "degraded",
            FeatureAvailability::Unavailable { .. } => "unavailable",
        }
    }
    /// Why a feature is degraded/unavailable (for introspection surfaces).
    pub fn reason(&self) -> Option<&str> {
        match self {
            FeatureAvailability::Available => None,
            FeatureAvailability::Degraded { reason }
            | FeatureAvailability::Unavailable { reason, .. } => Some(reason),
        }
    }
    /// How to make an unavailable feature available.
    pub fn fix(&self) -> Option<&str> {
        match self {
            FeatureAvailability::Unavailable { fix, .. } => Some(fix),
            _ => None,
        }
    }
}

/// Features whose behavior depends on the daemon. The set the capability model
/// reasons about — not an exhaustive list of editor features (the floor ones are
/// represented by [`DaemonFeature::LocalKb`] as the canonical "always works").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonFeature {
    // --- None: the in-process floor ---
    /// Local KB read/edit/agenda/search, LSP/DAP, AI, editing. Always works.
    LocalKb,
    // --- Recommends ---
    /// Hosting the primary KB on the daemon (thin client / shared backend).
    KbHosting,
    /// Persistence + background maintenance across editor lifecycles.
    CrossSessionPersistence,
    /// One backend serving N frontends (RAM/compute multiplier).
    MultiFrontendSharing,
    // --- Requires ---
    /// Sharing a KB with peers over the P2P mesh.
    P2pKbSharing,
    /// Continuous sync of a shared KB (prevents offline-edit divergence).
    ContinuousSharedSync,
    /// Mesh membership / trust operations.
    MeshMembership,
}

/// How to turn a daemon on, tailored to the configured mode. Shared by the
/// `fix` strings so the advice is consistent everywhere.
fn enable_daemon_hint(mode: DaemonMode) -> &'static str {
    match mode {
        DaemonMode::Off => {
            "set `daemon_mode` to `on-demand` (`:set daemon-mode on-demand`) and restart, \
             or start one with `mae setup-daemon`"
        }
        // Configured for a daemon but none is reachable.
        DaemonMode::OnDemand | DaemonMode::Shared => {
            "start the daemon (`mae setup-daemon`); it should auto-attach"
        }
    }
}

impl DaemonFeature {
    /// Every modeled feature, for enumerating availability (introspection).
    pub const ALL: [DaemonFeature; 7] = [
        DaemonFeature::LocalKb,
        DaemonFeature::KbHosting,
        DaemonFeature::CrossSessionPersistence,
        DaemonFeature::MultiFrontendSharing,
        DaemonFeature::P2pKbSharing,
        DaemonFeature::ContinuousSharedSync,
        DaemonFeature::MeshMembership,
    ];

    /// Stable kebab-case id used by `(feature-available? …)` + the MCP tool.
    pub fn id(self) -> &'static str {
        match self {
            DaemonFeature::LocalKb => "local-kb",
            DaemonFeature::KbHosting => "kb-hosting",
            DaemonFeature::CrossSessionPersistence => "cross-session-persistence",
            DaemonFeature::MultiFrontendSharing => "multi-frontend-sharing",
            DaemonFeature::P2pKbSharing => "p2p-sharing",
            DaemonFeature::ContinuousSharedSync => "continuous-sync",
            DaemonFeature::MeshMembership => "mesh-membership",
        }
    }

    /// Parse a feature id (accepts `_` or `-`), for the Scheme/AI surfaces.
    pub fn from_id(s: &str) -> Option<DaemonFeature> {
        let norm = s.trim().to_ascii_lowercase().replace('_', "-");
        DaemonFeature::ALL.into_iter().find(|f| f.id() == norm)
    }

    /// This feature's tier.
    pub fn requirement(self) -> DaemonRequirement {
        match self {
            DaemonFeature::LocalKb => DaemonRequirement::None,
            DaemonFeature::KbHosting
            | DaemonFeature::CrossSessionPersistence
            | DaemonFeature::MultiFrontendSharing => DaemonRequirement::Recommends,
            DaemonFeature::P2pKbSharing
            | DaemonFeature::ContinuousSharedSync
            | DaemonFeature::MeshMembership => DaemonRequirement::Requires,
        }
    }

    /// A short human label for surfaces.
    pub fn label(self) -> &'static str {
        match self {
            DaemonFeature::LocalKb => "local KB / editing",
            DaemonFeature::KbHosting => "KB hosting",
            DaemonFeature::CrossSessionPersistence => "cross-session persistence",
            DaemonFeature::MultiFrontendSharing => "multi-frontend sharing",
            DaemonFeature::P2pKbSharing => "P2P KB sharing",
            DaemonFeature::ContinuousSharedSync => "continuous shared-KB sync",
            DaemonFeature::MeshMembership => "mesh membership",
        }
    }

    /// Evaluate availability against the current daemon state. Pure — the single
    /// gate every surface consults before acting.
    pub fn availability(self, state: &DaemonStateSnapshot) -> FeatureAvailability {
        match self.requirement() {
            // The floor always works.
            DaemonRequirement::None => FeatureAvailability::Available,

            // Better with a daemon; degraded (still usable) without one.
            DaemonRequirement::Recommends => {
                if state.daemon_present() {
                    FeatureAvailability::Available
                } else {
                    FeatureAvailability::Degraded {
                        reason: format!(
                            "{} works locally but is better with a daemon ({})",
                            self.label(),
                            enable_daemon_hint(state.mode)
                        ),
                    }
                }
            }

            // Hard dependency: needs the right daemon state.
            DaemonRequirement::Requires => {
                // P2P / mesh ops need the control channel.
                let needs_control = matches!(
                    self,
                    DaemonFeature::P2pKbSharing | DaemonFeature::MeshMembership
                );
                // Continuous sync needs a live collab session, not just a daemon.
                let needs_collab = matches!(self, DaemonFeature::ContinuousSharedSync);

                if needs_control && !state.control_wired {
                    return FeatureAvailability::Unavailable {
                        reason: format!("{} needs a daemon with P2P enabled", self.label()),
                        fix: format!(
                            "{}, then enable P2P with `mae setup-collab --p2p`",
                            enable_daemon_hint(state.mode)
                        ),
                    };
                }
                if needs_collab && !state.collab_connected {
                    return FeatureAvailability::Unavailable {
                        reason: format!(
                            "{} needs a connected daemon to keep peers in sync",
                            self.label()
                        ),
                        fix: format!(
                            "{} and connect (`:collab-connect`); offline edits stay durable \
                             and converge on reconnect",
                            enable_daemon_hint(state.mode)
                        ),
                    };
                }
                FeatureAvailability::Available
            }
        }
    }
}

impl super::Editor {
    /// Snapshot the current daemon + collab state for capability evaluation. The
    /// single place surfaces read daemon state from, so gating, UX, and
    /// introspection all agree.
    pub fn daemon_state_snapshot(&self) -> DaemonStateSnapshot {
        DaemonStateSnapshot {
            mode: self.kb.daemon_mode,
            control_wired: self.kb.has_daemon_control(),
            read_layer_up: self.kb.has_daemon(),
            collab_connected: matches!(self.collab.status, super::CollabStatus::Connected { .. }),
            hosting_primary: self.kb.daemon_hosts_primary(),
        }
    }

    /// Evaluate whether `feature` is usable right now — the gate every surface
    /// (command, Scheme, AI) calls before acting, so a `Requires` feature shows an
    /// actionable reason instead of a silently failed call.
    pub fn feature_availability(&self, feature: DaemonFeature) -> FeatureAvailability {
        feature.availability(&self.daemon_state_snapshot())
    }

    /// JSON snapshot of daemon state + per-feature availability. The SAME data the
    /// `(daemon-status)` Scheme primitive and the `daemon_status` MCP tool expose
    /// (CLAUDE.md #3 the AI is a peer, #7 Scheme-accessible). `{}` on failure.
    pub fn daemon_status_json(&self) -> String {
        let snap = self.daemon_state_snapshot();
        let features: Vec<serde_json::Value> = DaemonFeature::ALL
            .into_iter()
            .map(|f| {
                let av = f.availability(&snap);
                serde_json::json!({
                    "id": f.id(),
                    "label": f.label(),
                    "requirement": f.requirement().as_str(),
                    "availability": av.label(),
                    "reason": av.reason(),
                    "fix": av.fix(),
                })
            })
            .collect();
        serde_json::json!({
            "mode": snap.mode.as_str(),
            "daemon_present": snap.daemon_present(),
            "control_wired": snap.control_wired,
            "read_layer_up": snap.read_layer_up,
            "collab_connected": snap.collab_connected,
            "hosting_primary": snap.hosting_primary,
            "features": features,
        })
        .to_string()
    }

    /// Is a daemon present (control or read layer) right now? Backs
    /// `(daemon-available?)`.
    pub fn daemon_available(&self) -> bool {
        self.daemon_state_snapshot().daemon_present()
    }

    /// Availability of a single feature by id, as a JSON object
    /// (`{available, requirement, reason, fix}`) — backs `(feature-available? id)`
    /// + the MCP tool. Returns an `error` object for an unknown id.
    pub fn feature_availability_json(&self, feature_id: &str) -> String {
        match DaemonFeature::from_id(feature_id) {
            Some(f) => {
                let av = self.feature_availability(f);
                serde_json::json!({
                    "id": f.id(),
                    "requirement": f.requirement().as_str(),
                    "available": av.is_available(),
                    "availability": av.label(),
                    "reason": av.reason(),
                    "fix": av.fix(),
                })
                .to_string()
            }
            None => {
                let known: Vec<&str> = DaemonFeature::ALL.iter().map(|f| f.id()).collect();
                serde_json::json!({
                    "error": format!("unknown feature '{feature_id}'"),
                    "known": known,
                })
                .to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(
        mode: DaemonMode,
        control: bool,
        read: bool,
        collab: bool,
        hosting: bool,
    ) -> DaemonStateSnapshot {
        DaemonStateSnapshot {
            mode,
            control_wired: control,
            read_layer_up: read,
            collab_connected: collab,
            hosting_primary: hosting,
        }
    }

    #[test]
    fn floor_always_available() {
        let off = snap(DaemonMode::Off, false, false, false, false);
        assert_eq!(
            DaemonFeature::LocalKb.availability(&off),
            FeatureAvailability::Available
        );
        assert_eq!(
            DaemonFeature::LocalKb.requirement(),
            DaemonRequirement::None
        );
    }

    #[test]
    fn recommends_degrades_without_daemon_available_with() {
        let none = snap(DaemonMode::Off, false, false, false, false);
        assert!(matches!(
            DaemonFeature::KbHosting.availability(&none),
            FeatureAvailability::Degraded { .. }
        ));
        // With a read layer up, it's available.
        let up = snap(DaemonMode::OnDemand, false, true, false, false);
        assert!(DaemonFeature::KbHosting.availability(&up).is_available());
    }

    #[test]
    fn requires_p2p_needs_control_channel() {
        let no_daemon = snap(DaemonMode::Off, false, false, false, false);
        match DaemonFeature::P2pKbSharing.availability(&no_daemon) {
            FeatureAvailability::Unavailable { fix, .. } => {
                assert!(fix.contains("setup-collab --p2p"), "fix: {fix}");
                assert!(fix.contains("daemon_mode") || fix.contains("setup-daemon"));
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
        // Control wired → available.
        let wired = snap(DaemonMode::Shared, true, true, true, false);
        assert!(DaemonFeature::P2pKbSharing
            .availability(&wired)
            .is_available());
    }

    #[test]
    fn continuous_sync_needs_live_collab() {
        // Control wired but collab not connected → unavailable (offline edits would
        // diverge until reconnect).
        let offline = snap(DaemonMode::Shared, true, true, false, false);
        assert!(matches!(
            DaemonFeature::ContinuousSharedSync.availability(&offline),
            FeatureAvailability::Unavailable { .. }
        ));
        let online = snap(DaemonMode::Shared, true, true, true, false);
        assert!(DaemonFeature::ContinuousSharedSync
            .availability(&online)
            .is_available());
    }
}
