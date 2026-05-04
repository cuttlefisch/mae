//! Highlight spans for the agenda buffer.

use crate::agenda_view::AgendaLineKind;
use crate::buffer::Buffer;
use crate::buffer_view::BufferView;
use crate::syntax::HighlightSpan;

/// Compute highlight spans for an agenda buffer by mapping `AgendaLineKind`
/// to theme keys. TODO states and priorities get distinct colors.
pub fn compute_agenda_spans(buf: &Buffer) -> Vec<HighlightSpan> {
    let view = match &buf.view {
        BufferView::Agenda(v) => v,
        _ => return Vec::new(),
    };

    let mut spans = Vec::new();
    let rope = buf.rope();
    let mut byte_offset = 0usize;

    for (i, line) in view.lines.iter().enumerate() {
        let line_len = if i < rope.len_lines() {
            rope.line(i).len_bytes()
        } else {
            0
        };

        match &line.kind {
            AgendaLineKind::Header => {
                spans.push(HighlightSpan {
                    byte_start: byte_offset,
                    byte_end: byte_offset + line.text.len(),
                    theme_key: "markup.heading",
                });
            }
            AgendaLineKind::TodoItem { state, priority } => {
                // Color the TODO state keyword.
                if let Some(state_start) = line.text.find(state.as_str()) {
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
                    if let Some(pos) = line.text.find(&marker) {
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
