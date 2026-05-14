//! Internal backend — scheme/elisp blocks routed to editor runtime.

use std::path::Path;

use super::LanguageBackend;
use crate::execute::ExecResult;
use crate::SrcBlock;

pub struct InternalBackend;

impl LanguageBackend for InternalBackend {
    fn name(&self) -> &str {
        "internal"
    }

    fn can_handle(&self, language: &str) -> bool {
        matches!(language, "scheme" | "elisp" | "emacs-lisp")
    }

    fn execute(&mut self, block: &SrcBlock, _dir: &Path, _vars: &[(String, String)]) -> ExecResult {
        ExecResult::PendingSchemeEval(block.body.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeaderArgs;

    #[test]
    fn handles_scheme() {
        let b = InternalBackend;
        assert!(b.can_handle("scheme"));
        assert!(b.can_handle("elisp"));
        assert!(!b.can_handle("python"));
    }

    #[test]
    fn returns_pending_eval() {
        let mut b = InternalBackend;
        let block = SrcBlock {
            name: None,
            language: "scheme".to_string(),
            header_args: HeaderArgs::default(),
            body: "(+ 1 2)".to_string(),
            line_range: (0, 2),
            body_byte_range: (0, 7),
        };
        let result = b.execute(&block, Path::new("/tmp"), &[]);
        match result {
            ExecResult::PendingSchemeEval(code) => assert_eq!(code, "(+ 1 2)"),
            other => panic!("Expected PendingSchemeEval, got {:?}", other),
        }
    }
}
