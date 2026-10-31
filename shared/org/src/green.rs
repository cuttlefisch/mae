//! Events to a rowan green tree. Headline lines are cut here, once the
//! document's TODO keywords are known, and inline objects are parsed here.

use crate::headline;
use crate::inline::{self, radio::Radio, InlineCx};
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
    // Radio targets are found with no radio links active, as Org finds them
    // when the buffer is set up; then every container is parsed with them.
    let radio = Radio::find(cx, |cx, table| {
        expand(cx, events, todo, table, &mut |_| {});
    });
    let cx = &InlineCx {
        radio: radio.as_ref(),
        ..*cx
    };
    let mut b = GreenNodeBuilder::new();
    let text = cx.src.text;
    expand(cx, events, todo, contents, &mut |ev| {
        simple(&mut b, text, ev)
    });
    b.finish()
}

/// The events with every headline line cut and every object container
/// parsed, handed to `sink` in order.
fn expand(
    cx: &InlineCx,
    events: &[Event],
    todo: &TodoKeywords,
    contents: &mut ContentsTable,
    sink: &mut dyn FnMut(&Event),
) {
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
                        objects.iter().for_each(&mut *sink);
                    } else {
                        sink(&Event::Token(k, s + a, s + z));
                    }
                }
            }
            Event::Inline(s, e, container, budget) => {
                objects.clear();
                inline::parse(cx, s, e, container, budget, &mut objects, contents);
                objects.iter().for_each(&mut *sink);
            }
            ref other => sink(other),
        }
    }
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
