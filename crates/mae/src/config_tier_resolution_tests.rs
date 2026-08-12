//! ADR-096 Phase 1 / ADR-084 D7 — Gate 1: Scheme participates in tier resolution.
//!
//! Split out of `config.rs` to stay under the structural ceiling, following
//! `config_template_tests.rs`.
//!
//! The oracle discipline here matters more than usual. `ai_tier` was a *decoy*
//! (#640): it was registered, guarded, persisted, and answered successfully while
//! reaching nothing. A test that set the option and read it back would have passed
//! against the decoy — reading back what you just wrote is exactly what made it
//! look like it worked. So every test below asserts on the **resolved policy**, and
//! the headline one asserts on a **dispatch decision**.

use super::*;
use mae_ai::{Decision, PermissionPolicy, PermissionTier};

/// Editor with `ai_tier` explicitly set through the real option path — the same
/// chokepoint `(set-option! "ai_tier" …)` in init.scm and `:set ai-tier` use, so
/// this exercises what a user actually does rather than poking the field.
fn editor_with_scheme_tier(tier: &str) -> mae_core::Editor {
    let mut editor = mae_core::Editor::new();
    editor
        .set_option("ai_tier", tier)
        .unwrap_or_else(|e| panic!("set_option(ai_tier, {tier:?}) failed: {e}"));
    editor
}

fn toml_with_tier(tier: Option<&str>) -> Config {
    let mut cfg = Config::default();
    cfg.ai.auto_approve_tier = tier.map(|t| t.to_string());
    cfg
}

/// **The headline gate.** Setting the tier from Scheme must change what the
/// enforcement point *decides*, not what the option reports.
///
/// Oracle: a tool that is `Ask` at the shipped default becomes `Allow`. Asserting
/// `get_option("ai_tier") == "shell"` would have passed against the decoy for its
/// entire existence.
#[test]
fn ai_tier_set_from_scheme_changes_the_enforced_policy() {
    let _lock = mae_effect_sandbox::lock_env();
    std::env::remove_var("MAE_AI_PERMISSIONS");

    // Baseline: at the shipped default, a Shell-tier call is asked, not allowed.
    let default_policy = PermissionPolicy::default();
    assert_eq!(
        default_policy.decide_tier(PermissionTier::Shell),
        Decision::Ask,
        "precondition: the shipped default must ASK for a Shell-tier call"
    );

    let editor = editor_with_scheme_tier("shell");
    let scheme = SchemeAiOverrides::from_editor(&editor);
    let resolved = resolve_permission_policy_with_scheme(&toml_with_tier(None), &scheme)
        .expect("a Scheme-set tier must resolve");

    assert_eq!(
        resolved.decide_tier(PermissionTier::Shell),
        Decision::Allow,
        "a Scheme-set tier did not reach the enforced policy — this is the #640 decoy"
    );
}

/// Precedence, with every layer below set to a DIFFERENT value so a pass proves
/// ordering rather than coincidence. Mirrors the existing
/// `resolve_ai_config_with_scheme` precedence tests.
#[test]
fn tier_precedence_is_env_then_scheme_then_toml_then_default() {
    let _lock = mae_effect_sandbox::lock_env();

    let editor = editor_with_scheme_tier("write");
    let scheme = SchemeAiOverrides::from_editor(&editor);
    let toml = toml_with_tier(Some("privileged"));

    // env beats scheme beats toml
    std::env::set_var("MAE_AI_PERMISSIONS", "readonly");
    let p = resolve_permission_policy_with_scheme(&toml, &scheme).unwrap();
    assert_eq!(
        p.auto_approve_up_to,
        PermissionTier::ReadOnly,
        "env must outrank both Scheme (write) and config.toml (privileged)"
    );

    // scheme beats toml
    std::env::remove_var("MAE_AI_PERMISSIONS");
    let p = resolve_permission_policy_with_scheme(&toml, &scheme).unwrap();
    assert_eq!(
        p.auto_approve_up_to,
        PermissionTier::Write,
        "Scheme must outrank config.toml — init.scm is the primary surface (ADR-096)"
    );

    // toml beats default, when Scheme is untouched
    let untouched = SchemeAiOverrides::from_editor(&mae_core::Editor::new());
    let p = resolve_permission_policy_with_scheme(&toml, &untouched).unwrap();
    assert_eq!(
        p.auto_approve_up_to,
        PermissionTier::Privileged,
        "with no Scheme value, config.toml must still win over the built-in default"
    );

    // default, when nothing is set anywhere
    let p = resolve_permission_policy_with_scheme(&toml_with_tier(None), &untouched).unwrap();
    assert_eq!(
        p.auto_approve_up_to,
        PermissionPolicy::default().auto_approve_up_to,
        "unconfigured must resolve to the shipped default"
    );
}

/// **Gate 1b — the coincidence-proof case.** An option sitting at its registered
/// default must be distinguishable from one a user deliberately set to that same
/// value.
///
/// The value chosen is `readonly` — `ai_tier`'s OWN default — on purpose. If
/// explicit-set tracking were dropped and Scheme reported unconditionally, the
/// first assertion below would still pass by accident; only the second, where the
/// deliberate choice must beat a *different* config.toml value, can catch it.
#[test]
fn an_unset_option_is_distinguishable_from_one_set_to_its_default() {
    let _lock = mae_effect_sandbox::lock_env();
    std::env::remove_var("MAE_AI_PERMISSIONS");

    let toml = toml_with_tier(Some("shell"));

    // Untouched: config.toml wins.
    let untouched = SchemeAiOverrides::from_editor(&mae_core::Editor::new());
    assert!(
        untouched.opt("tier").is_none(),
        "an untouched ai_tier must report as UNSET, not as its default value"
    );
    let p = resolve_permission_policy_with_scheme(&toml, &untouched).unwrap();
    assert_eq!(
        p.auto_approve_up_to,
        PermissionTier::Shell,
        "with ai_tier untouched, config.toml must win"
    );

    // Explicitly set to its own default: Scheme wins, tightening below config.toml.
    let editor = editor_with_scheme_tier("readonly");
    let scheme = SchemeAiOverrides::from_editor(&editor);
    let p = resolve_permission_policy_with_scheme(&toml, &scheme).unwrap();
    assert_eq!(
        p.auto_approve_up_to,
        PermissionTier::ReadOnly,
        "an EXPLICIT ai_tier must outrank config.toml even when it equals the default"
    );
}

/// An unparseable Scheme tier must refuse, never resolve to something permissive
/// (CWE-636, ADR-084 D4). The option's own set path rejects unknown spellings, so
/// this asserts the resolver's behaviour directly for the case where a value
/// reaches it by another route.
#[test]
fn an_unparseable_scheme_tier_refuses_rather_than_guessing() {
    let _lock = mae_effect_sandbox::lock_env();
    std::env::remove_var("MAE_AI_PERMISSIONS");

    // a plausible typo
    let scheme = SchemeAiOverrides {
        tier: "shel".to_string(),
        ..Default::default()
    };

    let err = resolve_permission_policy_with_scheme(&toml_with_tier(None), &scheme)
        .expect_err("an unparseable tier must not resolve");
    assert!(
        err.contains("shel"),
        "the error must name the offending value: {err}"
    );

    // And the option surface refuses it too, rather than storing garbage.
    let mut editor = mae_core::Editor::new();
    assert!(
        editor.set_option("ai_tier", "shel").is_err(),
        "set_option must reject an unknown tier spelling"
    );
}

/// #640's other half: the option must accept every spelling the shared vocabulary
/// does. `:set ai-tier shell` — the spelling config.toml uses, `config_name()`
/// emits, and `mae-agent --permission-mode` takes — used to be rejected outright.
#[test]
fn ai_tier_option_accepts_every_spelling_the_parser_does() {
    for spelling in PermissionTier::VALID_SPELLINGS {
        let mut editor = mae_core::Editor::new();
        let set = editor.set_option("ai_tier", spelling);
        assert!(
            set.is_ok(),
            "the option rejected {spelling:?}, which PermissionTier::parse accepts"
        );
        // And it normalises, so every surface reads back one spelling.
        let expected = PermissionTier::parse(spelling).unwrap().config_name();
        let (got, _) = editor
            .get_option("ai_tier")
            .expect("ai_tier must read back");
        assert_eq!(
            got, expected,
            "setting {spelling:?} must store the canonical {expected:?}"
        );
    }
}
