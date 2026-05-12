//! mae-snippets: Snippet expansion engine with tab-stops, mirrors, and field navigation.
//!
//! @stability: experimental
//! @since: 0.9.0
//!
//! Parses VSCode/LSP-compatible snippet syntax and provides a session-based
//! expansion model where the user navigates between fields with Tab/S-Tab.

pub mod parser;
pub mod snippet;
pub mod store;

pub use parser::{parse_snippet, ParseError, SnippetPart};
pub use snippet::{SnippetField, SnippetSession};
pub use store::{SnippetDef, SnippetStore};
