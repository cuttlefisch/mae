//! Highlight spans for the agenda buffer.

use crate::agenda_view::AgendaLineKind;
use crate::buffer::Buffer;
use crate::buffer_view::BufferView;
use crate::syntax::HighlightSpan;

/// Compute highlight spans for an agenda buffer by mapping `AgendaLineKind`
/// to theme keys. TODO states and priorities get distinct colors.
///
/// ADR-087 domain note: unlike `compute_git_status_spans` /
/// `compute_notif_spans` / `compute_kb_sharing_spans` (which now share
/// `line_kind_spans::compute_line_kind_spans`), this function is NOT routed
/// through that helper — it produces *multiple sub-line spans per line*
/// (the TODO-state keyword and the priority marker highlighted separately
/// within a line), not one whole-line span, so it isn't a fourth copy of
/// the same shape.
///
/// It also works in a pure **byte** domain throughout: `byte_offset`
/// accumulates via `rope.line(i).len_bytes()`, and keyword positions come
/// from `str::find` (byte offsets) on the rope's own line content — not on
/// `AgendaLine::text` (a separately-held `String` the view keeps for
/// display), which a prior version of this function searched instead. That
/// was byte-correct only because `render_agenda_text` happens to join
/// those exact `text` fields into the buffer, so the two strings currently
/// agree; searching the rope directly instead makes that agreement a
/// non-issue rather than an assumption.
pub fn compute_agenda_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let view = match &buf.view {
        BufferView::Agenda(v) => v,
        _ => return Vec::new(),
    };

    let mut spans = Vec::new();
    let rope = buf.rope();
    let mut byte_offset = 0usize;

    for (i, line) in view.lines.iter().enumerate() {
        if i >= rope.len_lines() {
            break;
        }
        let rope_line = rope.line(i);
        let line_len = rope_line.len_bytes();

        // Content without the trailing newline, straight from the rope.
        let mut content = rope_line.to_string();
        if i + 1 < rope.len_lines() && content.ends_with('\n') {
            content.pop();
        }

        match &line.kind {
            AgendaLineKind::Header => {
                spans.push(HighlightSpan {
                    byte_start: byte_offset,
                    byte_end: byte_offset + content.len(),
                    theme_key: "markup.heading",
                });
            }
            AgendaLineKind::TodoItem { state, priority } => {
                // Color the TODO state keyword.
                if let Some(state_start) = content.find(state.as_str()) {
                    let theme = match state.as_str() {
                        "TODO" => "markup.todo.todo",
                        "DONE" => "markup.todo.done",
                        "NEXT" => "markup.todo.next",
                        "WAIT" => "markup.todo.wait",
                        _ => "markup.todo.todo",
                    };
                    spans.push(HighlightSpan {
                        byte_start: byte_offset + state_start,
                        byte_end: byte_offset + state_start + state.len(),
                        theme_key: theme,
                    });
                }
                // Color priority marker.
                if let Some(pri) = priority {
                    let marker = format!("[#{}]", pri);
                    if let Some(pos) = content.find(&marker) {
                        let theme = match pri {
                            'A' => "markup.priority.a",
                            'B' => "markup.priority.b",
                            _ => "markup.priority.c",
                        };
                        spans.push(HighlightSpan {
                            byte_start: byte_offset + pos,
                            byte_end: byte_offset + pos + marker.len(),
                            theme_key: theme,
                        });
                    }
                }
            }
            AgendaLineKind::Blank => {}
        }

        byte_offset += line_len;
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agenda_view::{AgendaFilter, AgendaLine, AgendaView};
    use crate::buffer::BufferKind;

    fn make_agenda_buffer(lines: Vec<AgendaLine>) -> Buffer {
        let text: String = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let mut buf = Buffer::new();
        buf.kind = BufferKind::Agenda;
        buf.insert_text_at(0, &text);
        buf.view = BufferView::Agenda(Box::new(AgendaView {
            lines,
            filter: AgendaFilter::default(),
        }));
        buf
    }

    #[test]
    fn agenda_spans_header() {
        let buf = make_agenda_buffer(vec![AgendaLine {
            text: "Agenda".to_string(),
            kind: AgendaLineKind::Header,
            node_id: None,
            source_file: None,
        }]);
        let spans = compute_agenda_spans(&buf);
        assert!(spans.iter().any(|s| s.theme_key == "markup.heading"));
    }

    #[test]
    fn agenda_spans_todo_state() {
        let buf = make_agenda_buffer(vec![AgendaLine {
            text: "  TODO Fix bug".to_string(),
            kind: AgendaLineKind::TodoItem {
                state: "TODO".to_string(),
                priority: None,
            },
            node_id: None,
            source_file: None,
        }]);
        let spans = compute_agenda_spans(&buf);
        assert!(spans.iter().any(|s| s.theme_key == "markup.todo.todo"));
    }

    #[test]
    fn agenda_spans_priority() {
        let buf = make_agenda_buffer(vec![AgendaLine {
            text: "  TODO [#A] Urgent".to_string(),
            kind: AgendaLineKind::TodoItem {
                state: "TODO".to_string(),
                priority: Some('A'),
            },
            node_id: None,
            source_file: None,
        }]);
        let spans = compute_agenda_spans(&buf);
        assert!(spans.iter().any(|s| s.theme_key == "markup.priority.a"));
    }
}
