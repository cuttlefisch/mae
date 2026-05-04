//! Agenda buffer — cross-file TODO view with filters.
//!
//! Queries the KB for nodes with `todo_state` set and presents them
//! in a filterable, navigable buffer.

/// Filter criteria for the agenda view.
#[derive(Debug, Clone, Default)]
pub struct AgendaFilter {
    pub todo_states: Option<Vec<String>>,
    pub priority: Option<char>,
    pub tag: Option<String>,
}

/// A single line in the agenda display.
#[derive(Debug, Clone)]
pub struct AgendaLine {
    pub text: String,
    pub kind: AgendaLineKind,
    pub node_id: Option<String>,
    pub source_file: Option<String>,
}

/// What kind of line this is (for rendering).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgendaLineKind {
    Header,
    TodoItem {
        state: String,
        priority: Option<char>,
    },
    Blank,
}

/// Full agenda view state.
#[derive(Debug, Clone)]
pub struct AgendaView {
    pub lines: Vec<AgendaLine>,
    pub filter: AgendaFilter,
}

impl AgendaView {
    pub fn new(filter: AgendaFilter) -> Self {
        Self {
            lines: Vec::new(),
            filter,
        }
    }
}
