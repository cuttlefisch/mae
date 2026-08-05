//! #639 — the generated config template must not contradict the shipped
//! permission default.
//!
//! Split out of `config.rs` to stay under the structural ceiling. Both gates derive
//! the expected value from `PermissionPolicy::default()` rather than hardcoding
//! "readonly": a literal would pass forever while the shipped default moved beneath
//! it, which is exactly how the template came to advertise `shell` after ADR-090 D5.

use super::*;
use mae_ai::PermissionPolicy;

/// The tier line from the generated template, if any.
fn tier_lines(t: &str) -> Vec<String> {
    t.lines()
        .filter(|l| l.contains("auto_approve_tier") || l.contains("Tiers:"))
        .map(|l| l.trim().to_string())
        .collect()
}

/// #639 Gate. The template must name the tier MAE actually ships as the default.
///
/// The expected value is derived from `PermissionPolicy::default()` rather than
/// written here as a literal. That is the whole point: a test asserting the string
/// `"readonly"` would pass forever while the shipped default moved underneath it,
/// which is precisely how the template came to advertise `shell` as the default
/// after ADR-090 D5 changed it to `ReadOnly`.
#[test]
fn generated_template_states_the_shipped_default() {
    let shipped = PermissionPolicy::default().auto_approve_up_to.config_name();
    let template = default_config_template();
    let lines = tier_lines(&template);
    assert!(
        !lines.is_empty(),
        "template mentions no permission tier at all"
    );

    let joined = lines.join("\n");
    assert!(
        joined.contains(&format!("\"{shipped}\" (default)")),
        "the template must mark the SHIPPED default ({shipped}) as the default.\n\
         Found:\n{joined}"
    );

    // And it must not mark anything else as the default.
    for tier in ["readonly", "write", "shell", "privileged"] {
        if tier != shipped {
            assert!(
                !joined.contains(&format!("\"{tier}\" (default)")),
                "template calls {tier} the default, but the shipped default is \
                 {shipped}:\n{joined}"
            );
        }
    }
}

/// #639 Gate. The template must not hand the user a line that raises the tier.
///
/// ADR-090 rejected dropping the default without an Ask state precisely because
/// *"the predictable result is that users set `auto_approve_tier = \"shell\"` in
/// config — restoring the same posture while adding the false comfort of a
/// configured value."* Shipping a generator that pre-writes that line produces the
/// rejected outcome by default.
#[test]
fn template_offers_no_tier_above_the_default() {
    let shipped = PermissionPolicy::default().auto_approve_up_to;
    let template = default_config_template();

    for line in template.lines() {
        let Some(rest) = line.split_once("auto_approve_tier").map(|(_, r)| r) else {
            continue;
        };
        let Some(quoted) = rest.split('"').nth(1) else {
            continue;
        };
        let offered = mae_ai::PermissionTier::parse(quoted)
            .unwrap_or_else(|| panic!("template offers an unparseable tier: {line}"));
        assert!(
            offered <= shipped,
            "the generated template offers `auto_approve_tier = \"{quoted}\"`, above \
             the shipped default ({}). A config generator must not pre-write an \
             escalation for the user — ADR-090 named that exact outcome as the \
             reason the default could not simply be lowered.",
            shipped.config_name()
        );
    }
}
