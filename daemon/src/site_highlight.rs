//! The live KB site's code highlighter (ADR-119 D12): mae-site's
//! [`Highlighter`] over the grammars the editor highlights with
//! (`mae-syntax`, with its `site-grammars`).
//!
//! A grammar's highlight configuration (its compiled queries, the expensive
//! part: up to tens of milliseconds) is built on its first use and kept for
//! the life of the process, one per grammar, shared by every derivation.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use mae_site::{CodeRole, CodeSpan, Highlighter, PlainBlock};
use mae_syntax::{Grammar, HighlightConfiguration, Role};

/// Tree-sitter highlighting with a configuration cache per grammar.
pub struct SyntaxHighlighter {
    configs: HashMap<Grammar, OnceLock<Option<HighlightConfiguration>>>,
}

impl SyntaxHighlighter {
    fn new() -> Self {
        SyntaxHighlighter {
            configs: Grammar::ALL.iter().map(|&g| (g, OnceLock::new())).collect(),
        }
    }

    fn config(&self, grammar: Grammar) -> Option<&HighlightConfiguration> {
        self.configs
            .get(&grammar)?
            .get_or_init(|| mae_syntax::build_configuration(grammar))
            .as_ref()
    }
}

impl Highlighter for SyntaxHighlighter {
    fn highlight(&self, language: &str, text: &str) -> Option<Vec<CodeSpan>> {
        let grammar = Grammar::from_name(language)?;
        let config = self.config(grammar)?;
        // tree-sitter-highlight never refuses a configuration this crate
        // built; if it did, the block reads plain rather than as unknown.
        let regions = mae_syntax::highlight(config, text).unwrap_or_default();
        Some(
            regions
                .into_iter()
                .filter_map(|r| {
                    Some(CodeSpan {
                        start: r.start,
                        end: r.end,
                        role: code_role(mae_syntax::role(r.name)?),
                    })
                })
                .collect(),
        )
    }
}

/// The registry's role as the site's class. One enum each because each
/// crate owns its half (what a capture means; which class the palette
/// colours), and this exhaustive match holds them in step: a role added to
/// either does not compile until it is placed here.
fn code_role(role: Role) -> CodeRole {
    match role {
        Role::Comment => CodeRole::Comment,
        Role::String => CodeRole::String,
        Role::Number => CodeRole::Number,
        Role::Constant => CodeRole::Constant,
        Role::Keyword => CodeRole::Keyword,
        Role::Operator => CodeRole::Operator,
        Role::Function => CodeRole::Function,
        Role::Type => CodeRole::Type,
        Role::Variable => CodeRole::Variable,
    }
}

/// The process's one highlighter.
pub fn shared() -> Arc<dyn Highlighter> {
    static ONE: OnceLock<Arc<SyntaxHighlighter>> = OnceLock::new();
    ONE.get_or_init(|| Arc::new(SyntaxHighlighter::new()))
        .clone()
}

/// The pages a log line names at most; the rest are counted.
const PAGES_NAMED: usize = 10;

/// One log line per reason and language for the blocks one derivation of
/// `kb_id` left plain (P-108), naming the pages. Language and page IDs come
/// from content, so they are written quoted and escaped (`{:?}`): no line
/// break or control character reaches the log.
pub fn plain_block_lines(
    kb_id: &str,
    blocks: &std::collections::BTreeSet<PlainBlock>,
) -> Vec<String> {
    let mut by: BTreeMap<(&str, Option<&str>), Vec<&str>> = BTreeMap::new();
    for b in blocks {
        by.entry((b.reason.as_str(), b.language.as_deref()))
            .or_default()
            .push(&b.page);
    }
    by.into_iter()
        .map(|((reason, language), pages)| {
            let named: Vec<String> = pages.iter().take(PAGES_NAMED).map(|p| format!("{p:?}")).collect();
            let more = pages.len().saturating_sub(PAGES_NAMED);
            let more = if more > 0 { format!(" and {more} more") } else { String::new() };
            format!(
                "site: kb {kb_id:?}: source blocks left plain ({reason}), language {}: pages {}{more}",
                language.map_or_else(|| "(none)".to_string(), |l| format!("{l:?}")),
                named.join(", "),
            )
        })
        .collect()
}
