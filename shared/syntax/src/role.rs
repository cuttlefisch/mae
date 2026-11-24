//! Which of the site palette's code colours each highlight name takes.

/// The KB site palette's token roles (`--code-<role>`, class
/// `org-code-<role>`; palette/src/tokens.ts, group `code`). Text with no role
/// takes the block's text colour (`--code-text`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Comment,
    String,
    Number,
    Constant,
    Keyword,
    Operator,
    Function,
    Type,
    Variable,
}

impl Role {
    /// Every role, in the palette's order.
    pub const ALL: [Role; 9] = [
        Role::Comment,
        Role::String,
        Role::Number,
        Role::Constant,
        Role::Keyword,
        Role::Operator,
        Role::Function,
        Role::Type,
        Role::Variable,
    ];

    /// The role's name in the palette (`--code-<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Role::Comment => "comment",
            Role::String => "string",
            Role::Number => "number",
            Role::Constant => "constant",
            Role::Keyword => "keyword",
            Role::Operator => "operator",
            Role::Function => "function",
            Role::Type => "type",
            Role::Variable => "variable",
        }
    }
}

/// Every name in [`HIGHLIGHT_NAMES`](crate::HIGHLIGHT_NAMES) and its role; `None` is the text colour,
/// chosen, not defaulted. One entry per name, in that list's order (a test
/// holds the two in step).
pub const ROLES: &[(&str, Option<Role>)] = &[
    // Rust's `#[...]`, decorators, nginx's block attributes: directives to
    // the compiler or server, read like keywords.
    ("attribute", Some(Role::Keyword)),
    ("boolean", Some(Role::Constant)),
    ("comment", Some(Role::Comment)),
    ("constant", Some(Role::Constant)),
    ("constant.builtin", Some(Role::Constant)),
    // A constructor names its type (`Some`, `Vec::new`); the editor's themes
    // colour it as a type too.
    ("constructor", Some(Role::Type)),
    // An injected region: the injected language's own captures would colour
    // it, and injections are not resolved, so it is text.
    ("embedded", None),
    // An escape stands out inside its string, as the editor's themes show it.
    ("escape", Some(Role::Constant)),
    ("function", Some(Role::Function)),
    ("function.builtin", Some(Role::Function)),
    // Macros are called like functions (`println!`).
    ("function.macro", Some(Role::Function)),
    ("function.method", Some(Role::Function)),
    ("keyword", Some(Role::Keyword)),
    // Loop and goto labels: rare, and no role fits; text.
    ("label", None),
    // Module paths read as type-like names (`std::fmt`).
    ("namespace", Some(Role::Type)),
    ("number", Some(Role::Number)),
    ("operator", Some(Role::Operator)),
    // Fields and keys name a value, as a variable does (YAML keys, struct
    // fields).
    ("property", Some(Role::Variable)),
    // Brackets and separators are structure, not tokens to tell apart:
    // colouring every comma and brace is noise. Text.
    ("punctuation", None),
    ("punctuation.bracket", None),
    ("punctuation.delimiter", None),
    // `${` in a shell string, a template's interpolation: it changes how what
    // follows reads, as an operator does.
    ("punctuation.special", Some(Role::Operator)),
    ("string", Some(Role::String)),
    ("string.escape", Some(Role::Constant)),
    ("string.special", Some(Role::String)),
    // A JSON object's key names a field, as `property` does.
    ("string.special.key", Some(Role::Variable)),
    // Markup tags are the language's own syntax words.
    ("tag", Some(Role::Keyword)),
    // Emphasis is a font style; no colour role says it, and colouring it
    // would read as a token kind it is not. Text.
    ("text.emphasis", None),
    // Inline code and code spans read as literal text.
    ("text.literal", Some(Role::String)),
    // A link's target and an address are literals.
    ("text.reference", Some(Role::String)),
    ("text.strong", None),
    // A heading in a Markdown block is its structure's keyword.
    ("text.title", Some(Role::Keyword)),
    ("text.uri", Some(Role::String)),
    ("type", Some(Role::Type)),
    ("type.builtin", Some(Role::Type)),
    ("variable", Some(Role::Variable)),
    // `self`, `$HOME`: still variables, if the language's own.
    ("variable.builtin", Some(Role::Variable)),
    ("variable.parameter", Some(Role::Variable)),
];

/// The role of highlight name `name` (one of
/// [`HIGHLIGHT_NAMES`](crate::HIGHLIGHT_NAMES)); `None` for
/// text, and for a name not in that list.
pub fn role(name: &str) -> Option<Role> {
    ROLES.iter().find(|(n, _)| *n == name).and_then(|(_, r)| *r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HIGHLIGHT_NAMES;

    /// Every highlight name is in the table exactly once, in the same order,
    /// so each one maps by a decision written there: a name added to
    /// `HIGHLIGHT_NAMES` fails here until its role is chosen.
    #[test]
    fn every_highlight_name_has_a_chosen_role() {
        let table: Vec<&str> = ROLES.iter().map(|(n, _)| *n).collect();
        assert_eq!(table, HIGHLIGHT_NAMES);
        for (name, chosen) in ROLES {
            assert_eq!(role(name), *chosen, "{name}");
        }
        for unknown in ["", "keywordx", "Keyword", "comment.line", "markup"] {
            assert_eq!(role(unknown), None, "{unknown}");
        }
    }

    /// Every role is some name's: a palette colour nothing reaches would be
    /// a mapping mistake, not a choice.
    #[test]
    fn every_role_is_reached() {
        for r in Role::ALL {
            assert!(ROLES.iter().any(|(_, x)| *x == Some(r)), "{r:?}");
        }
        let names: Vec<&str> = Role::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(
            names,
            [
                "comment", "string", "number", "constant", "keyword", "operator", "function",
                "type", "variable"
            ]
        );
    }
}
