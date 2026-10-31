//! Events to a rowan green tree. Headline lines are cut here, once the
//! document's TODO keywords are known.

use crate::headline;
use crate::kind::OrgLanguage;
use crate::options::TodoKeywords;
use crate::tokens::Event;
use rowan::{GreenNode, GreenNodeBuilder, Language};

pub(crate) fn to_green(text: &str, events: &[Event], todo: &TodoKeywords) -> GreenNode {
    let mut b = GreenNodeBuilder::new();
    let raw = OrgLanguage::kind_to_raw;
    for ev in events {
        match *ev {
            Event::Start(k) => b.start_node(raw(k)),
            Event::Finish => b.finish_node(),
            Event::Token(k, s, e) => b.token(raw(k), &text[s..e]),
            Event::HeadlineLine(s, e) => {
                for (k, a, z) in headline::tokens(&text[s..e], todo) {
                    b.token(raw(k), &text[s + a..s + z]);
                }
            }
        }
    }
    b.finish()
}
