//! Model pricing table — USD per million tokens.
//!
//! Used by the session budget tracker to turn raw token counts into a
//! running cost estimate. The table is pattern-matched by prefix so new
//! point releases of a model family (e.g. `claude-sonnet-4-5-20251022`)
//! keep working without an entry update; unknown models return `None` so
//! callers can warn-and-pass-through rather than blocking the request.
//!
//! # Why this lives in the AI crate
//! Pricing is provider-adjacent by nature — cache-read vs. cache-write
//! rates vary by model AND by provider. Keeping this alongside the
//! provider impls means the table stays self-contained and the editor
//! crate never has to care about model identifiers.
//!
//! # Unknown models
//!
//! Models not in the table (e.g. Ollama-hosted local models like `llama3`
//! or `qwen2.5-coder:7b`) return `None` from [`lookup()`]. The session
//! budget tracker treats these as free/unmetered — budget enforcement is
//! skipped entirely rather than blocking the request. This is intentional:
//! a local FOSS contributor should never hit a budget rejection.
//!
//! # Adding new model families
//!
//! Add a `(prefix, ModelPrice)` entry to [`TABLE`]. Order matters: longer
//! prefixes must come before shorter ones so the first match wins (e.g.
//! `"gpt-4o-mini"` before `"gpt-4o"`). Group entries by provider and
//! sort within each group by specificity (most specific first).
//!
//! # Keeping rates fresh
//! Rates reflect published public pricing as of 2026-04. The source of
//! truth is each provider's pricing page. When rates change, update
//! both the table and the `snapshot_date` string below.

use serde::{Deserialize, Serialize};

/// Per-million-token rates for a single model. Cached input and cache-write
/// tiers are optional — not all models charge them separately, and not all
/// callers care (we only track "standard" input/output in the session
/// tracker; cached hits get charged at `input_per_mtok` which over-estimates
/// cost, erring safe).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    /// Standard input rate.
    pub input_per_mtok: f64,
    /// Output rate.
    pub output_per_mtok: f64,
}

impl ModelPrice {
    /// Estimated cost in USD for a single request.
    pub fn cost_usd(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        (input_tokens as f64 / 1_000_000.0) * self.input_per_mtok
            + (output_tokens as f64 / 1_000_000.0) * self.output_per_mtok
    }
}

/// Published snapshot date — bump when rates are refreshed.
pub const PRICING_SNAPSHOT: &str = "2026-04";

/// Look up a model's pricing. Matches by longest prefix: if no exact
/// entry exists for `claude-sonnet-4-5-20251022` we still hit the
/// `claude-sonnet-4` family rate. Returns `None` for unknown models
/// (e.g. Ollama / local models) — callers should treat that as "free"
/// and skip budget enforcement rather than blocking.
pub fn lookup(model: &str) -> Option<ModelPrice> {
    let lower = model.to_ascii_lowercase();
    // Longest-prefix wins. Ordered by specificity.
    for (prefix, price) in TABLE {
        if lower.starts_with(prefix) {
            return Some(*price);
        }
    }
    None
}

/// Published rates. Order matters — longer prefixes must come first so
/// the first match wins. Keep sorted within each provider by specificity.
const TABLE: &[(&str, ModelPrice)] = &[
    // ---- Anthropic ----
    // Claude Opus 4.6 (and 4.x opus family)
    (
        "claude-opus-4",
        ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        },
    ),
    // Claude Sonnet 4.x (4-5, 4-6, dated revisions)
    (
        "claude-sonnet-4",
        ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        },
    ),
    // Claude Haiku 4.5 — cheapest Anthropic tier for tool-loop testing
    (
        "claude-haiku-4",
        ModelPrice {
            input_per_mtok: 1.0,
            output_per_mtok: 5.0,
        },
    ),
    // Legacy Claude 3.x families — kept for back-compat with config files.
    (
        "claude-3-5-sonnet",
        ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
        },
    ),
    // Anthropic revised 3.5-haiku upward from $0.80/$4.0 shortly after
    // launch — use the current published rate so the budget tracker
    // errs on the safe (overestimate) side if a user pins this model.
    (
        "claude-3-5-haiku",
        ModelPrice {
            input_per_mtok: 1.0,
            output_per_mtok: 5.0,
        },
    ),
    (
        "claude-3-opus",
        ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 75.0,
        },
    ),
    // ---- OpenAI ----
    // gpt-4o-mini is the cheap dev default
    (
        "gpt-4o-mini",
        ModelPrice {
            input_per_mtok: 0.15,
            output_per_mtok: 0.60,
        },
    ),
    (
        "gpt-4o",
        ModelPrice {
            input_per_mtok: 2.50,
            output_per_mtok: 10.0,
        },
    ),
    (
        "gpt-4-turbo",
        ModelPrice {
            input_per_mtok: 10.0,
            output_per_mtok: 30.0,
        },
    ),
    (
        "gpt-4",
        ModelPrice {
            input_per_mtok: 30.0,
            output_per_mtok: 60.0,
        },
    ),
    (
        "o1-mini",
        ModelPrice {
            input_per_mtok: 3.0,
            output_per_mtok: 12.0,
        },
    ),
    (
        "o1",
        ModelPrice {
            input_per_mtok: 15.0,
            output_per_mtok: 60.0,
        },
    ),
    // ---- Gemini ----
    // Gemini 3.1 Pro (Preview)
    (
        "gemini-3.1-pro",
        ModelPrice {
            input_per_mtok: 2.0,
            output_per_mtok: 12.0,
        },
    ),
    // Gemini 3.1 Flash-Lite
    (
        "gemini-3.1-flash-lite",
        ModelPrice {
            input_per_mtok: 0.25,
            output_per_mtok: 1.50,
        },
    ),
    // Gemini 3.0 Flash (Preview)
    (
        "gemini-3.0-flash",
        ModelPrice {
            input_per_mtok: 0.50,
            output_per_mtok: 3.0,
        },
    ),
    // Gemini 2.5 Pro (Stable)
    (
        "gemini-2.5-pro",
        ModelPrice {
            input_per_mtok: 1.25,
            output_per_mtok: 10.0,
        },
    ),
    // Gemini 2.5 Flash
    (
        "gemini-2.5-flash",
        ModelPrice {
            input_per_mtok: 0.30,
            output_per_mtok: 2.50,
        },
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_exact_claude_sonnet() {
        let p = lookup("claude-sonnet-4-5").unwrap();
        assert_eq!(p.input_per_mtok, 3.0);
        assert_eq!(p.output_per_mtok, 15.0);
    }

    #[test]
    fn matches_dated_revision_via_prefix() {
        // Real model ids include date suffixes; the prefix match must
        // still hit.
        let p = lookup("claude-haiku-4-5-20251001").unwrap();
        assert_eq!(p.input_per_mtok, 1.0);
    }

    #[test]
    fn case_insensitive() {
        assert!(lookup("Claude-Opus-4-6").is_some());
        assert!(lookup("GPT-4O").is_some());
    }

    #[test]
    fn unknown_returns_none() {
        // Ollama local models aren't priced — we want a clean None so
        // the session tracker can short-circuit budget checks.
        assert!(lookup("llama3").is_none());
        assert!(lookup("qwen2.5-coder:7b").is_none());
    }

    #[test]
    fn cost_calculation_matches_published_rate() {
        let sonnet = lookup("claude-sonnet-4-5").unwrap();
        // 10k input + 1k output at $3/$15 per Mtok:
        // 10_000/1_000_000 * 3 + 1_000/1_000_000 * 15 = 0.03 + 0.015 = 0.045
        let cost = sonnet.cost_usd(10_000, 1_000);
        assert!((cost - 0.045).abs() < 1e-9);
    }

    #[test]
    fn haiku_cheaper_than_sonnet_cheaper_than_opus() {
        let h = lookup("claude-haiku-4-5").unwrap();
        let s = lookup("claude-sonnet-4-5").unwrap();
        let o = lookup("claude-opus-4-6").unwrap();
        assert!(h.cost_usd(1000, 1000) < s.cost_usd(1000, 1000));
        assert!(s.cost_usd(1000, 1000) < o.cost_usd(1000, 1000));
    }

    #[test]
    fn gpt4o_mini_is_dev_friendly() {
        let p = lookup("gpt-4o-mini").unwrap();
        assert!(
            p.input_per_mtok < 1.0,
            "gpt-4o-mini is supposed to be the cheap tier"
        );
    }

    #[test]
    fn zero_tokens_zero_cost() {
        let p = lookup("claude-haiku-4-5").unwrap();
        assert_eq!(p.cost_usd(0, 0), 0.0);
    }
}
