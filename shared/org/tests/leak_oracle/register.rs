//! The divergence register (`conformance/render-divergences.toml`) as the
//! judge reads it: which Emacs export errors MAE may render through, and
//! the diagnostic MAE must then carry. Nothing is hard-coded here: an
//! entry registers an error by quoting Org's message in its `org` field
//! (`the export fails: "Definition not found for footnote LABEL"`; an
//! upper-case placeholder word ends the quoted prefix), and names the
//! diagnostic kinds MAE reports for it in its `mae` field
//! (`footnote_undefined`).

use std::sync::OnceLock;

/// One registered divergence.
#[derive(Debug, Clone)]
pub struct Registered {
    pub id: String,
    /// Prefixes of the Emacs error messages it covers.
    pub messages: Vec<String>,
    /// Snake-case words of its `mae` field (diagnostic kinds among them).
    pub kinds: Vec<String>,
}

/// Every entry, read once from the register.
pub fn entries() -> &'static [Registered] {
    static R: OnceLock<Vec<Registered>> = OnceLock::new();
    R.get_or_init(|| {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/render-divergences.toml"
        );
        parse(&std::fs::read_to_string(path).expect("render-divergences.toml"))
    })
}

/// The entry covering Emacs error `error` for a render whose diagnostic
/// kinds are `diags`: one quoting the message, and naming one of `diags`
/// if it names any diagnostic kind MAE has.
pub fn lookup(error: &str, diags: &[&str]) -> Option<&'static Registered> {
    entries()
        .iter()
        .find(|r| r.messages.iter().any(|m| error.contains(m.as_str())) && r.satisfied_by(diags))
}

/// The entry quoting `error`, whatever MAE reported.
pub fn quoting(error: &str) -> Option<&'static Registered> {
    entries()
        .iter()
        .find(|r| r.messages.iter().any(|m| error.contains(m.as_str())))
}

impl Registered {
    /// Whether a render with diagnostic kinds `diags` carries what the
    /// entry names (an entry naming none of MAE's kinds asks nothing).
    pub fn satisfied_by(&self, diags: &[&str]) -> bool {
        let named: Vec<&String> = self.kinds.iter().filter(|k| is_kind(k)).collect();
        named.is_empty() || named.iter().any(|k| diags.contains(&k.as_str()))
    }
}

/// MAE's diagnostic kind names (`RenderDiagnostic::kind_name`) end in one
/// of these words; other snake-case words of a `mae` field are options.
fn is_kind(word: &str) -> bool {
    [
        "_undefined",
        "_attribute",
        "_environment",
        "_macro",
        "_deep",
        "unparsed",
    ]
    .iter()
    .any(|s| word.ends_with(s))
        || word.starts_with("macro_")
}

/// The entries of the register's text.
pub fn parse(text: &str) -> Vec<Registered> {
    let mut out: Vec<Registered> = Vec::new();
    for line in text.lines() {
        if line.trim() == "[[divergence]]" {
            out.push(Registered {
                id: String::new(),
                messages: Vec::new(),
                kinds: Vec::new(),
            });
            continue;
        }
        let (Some(r), Some((key, value))) = (out.last_mut(), line.split_once(" = ")) else {
            continue;
        };
        let value: String = serde_json::from_str(value.trim()).unwrap_or_default();
        match key.trim() {
            "id" => r.id = value,
            "org" => r.messages = quoted_prefixes(&value),
            "mae" => r.kinds = snake_words(&value),
            _ => {}
        }
    }
    out
}

/// The quoted strings of `s`, each cut at its first upper-case placeholder.
fn quoted_prefixes(s: &str) -> Vec<String> {
    s.split('"')
        .skip(1)
        .step_by(2)
        .map(|q| {
            let words: Vec<&str> = q
                .split(' ')
                .take_while(|w| !(w.len() >= 3 && w.chars().all(|c| c.is_ascii_uppercase())))
                .collect();
            words.join(" ")
        })
        .filter(|p| p.len() >= 8)
        .collect()
}

fn snake_words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_ascii_lowercase() || c == '_'))
        .filter(|w| w.contains('_') && !w.starts_with('_') && !w.ends_with('_'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_register_registers_the_footnote_and_babel_errors() {
        let fnote = quoting("(user-error \"Definition not found for footnote x\")");
        assert_eq!(fnote.map(|r| r.id.as_str()), Some("undefined-footnote"));
        assert!(lookup("Definition not found for footnote x", &[]).is_none());
        assert!(lookup(
            "Definition not found for footnote x",
            &["footnote_undefined"]
        )
        .is_some());
        let babel = lookup("(user-error \"Unknown Babel reference: nosuch\")", &[]);
        assert_eq!(babel.map(|r| r.id.as_str()), Some("call-unknown-reference"));
        assert!(quoting("(error \"Undefined Org macro: res; aborting\")").is_none());
    }
}
