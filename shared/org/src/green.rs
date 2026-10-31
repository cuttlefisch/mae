//! Events to a rowan green tree. Headline lines are cut here, once the
//! document's TODO keywords are known, and inline objects are parsed here.

use crate::headline;
use crate::inline::{self, InlineCx};
use crate::kind::{OrgLanguage, SyntaxKind};
use crate::options::TodoKeywords;
use crate::span::ContentsTable;
use crate::tokens::Event;
use rowan::{GreenNode, GreenNodeBuilder, Language};

pub(crate) fn to_green(
    cx: &InlineCx,
    events: &[Event],
    todo: &TodoKeywords,
    contents: &mut ContentsTable,
) -> GreenNode {
    let mut b = GreenNodeBuilder::new();
    let text = cx.src.text;
    let mut objects = Vec::new();
    for ev in events {
        match *ev {
            Event::HeadlineLine(s, e, budget) => {
                for (k, a, z) in headline::tokens(&text[s..e], todo) {
                    if k == SyntaxKind::TITLE {
                        objects.clear();
                        objects.push(Event::Start(SyntaxKind::HEADLINE_TITLE));
                        inline::parse(
                            cx,
                            s + a,
                            s + z,
                            SyntaxKind::HEADLINE_TITLE,
                            budget,
                            &mut objects,
                            contents,
                        );
                        objects.push(Event::Finish);
                        objects.iter().for_each(|o| simple(&mut b, text, o));
                    } else {
                        simple(&mut b, text, &Event::Token(k, s + a, s + z));
                    }
                }
            }
            Event::Inline(s, e, container, budget) => {
                objects.clear();
                inline::parse(cx, s, e, container, budget, &mut objects, contents);
                objects.iter().for_each(|o| simple(&mut b, text, o));
            }
            ref other => simple(&mut b, text, other),
        }
    }
    b.finish()
}

fn simple(b: &mut GreenNodeBuilder, text: &str, ev: &Event) {
    let raw = OrgLanguage::kind_to_raw;
    match *ev {
        Event::Start(k) => b.start_node(raw(k)),
        Event::Finish => b.finish_node(),
        Event::Token(k, s, e) => b.token(raw(k), &text[s..e]),
        Event::HeadlineLine(..) | Event::Inline(..) => {}
    }
}
