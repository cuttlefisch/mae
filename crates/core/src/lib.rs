pub mod buffer;
pub mod commands;
pub mod conversation;
pub mod debug;
pub mod editor;
pub mod file_picker;
pub mod grapheme;
pub mod keymap;
pub mod messages;
pub mod search;
pub mod theme;
pub mod window;
pub mod word;

pub use buffer::{Buffer, BufferKind};
pub use commands::{Command, CommandRegistry, CommandSource};
pub use debug::{Breakpoint, DebugState, DebugTarget, DebugThread, SchemeErrorEntry, Scope, StackFrame, Variable};
pub use conversation::Conversation;
pub use editor::{EditRecord, Editor};
pub use keymap::{Key, KeyPress, Keymap, LookupResult, WhichKeyEntry, parse_key_seq, parse_key_seq_spaced};
pub use messages::{LogEntry, MessageLevel, MessageLog, MessageLogHandle};
pub use theme::{BundledResolver, NamedColor, Theme, ThemeColor, ThemeError, ThemeResolver, ThemeStyle, bundled_theme_names, default_theme};
pub use search::{SearchDirection, SearchMatch, SearchState};
pub use file_picker::FilePicker;
pub use window::{Direction, LayoutNode, Rect as WinRect, SplitDirection, Window, WindowId, WindowManager};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualType {
    Char,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual(VisualType),
    Command,
    ConversationInput,
    Search,
    FilePicker,
}
