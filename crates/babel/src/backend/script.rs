//! Script backend — interpreted languages (python, ruby, node, etc.)

use std::path::Path;

use super::LanguageBackend;
use crate::execute::ExecResult;
use crate::SrcBlock;

pub struct ScriptBackend;

impl LanguageBackend for ScriptBackend {
    fn name(&self) -> &str {
        "script"
    }

    fn can_handle(&self, language: &str) -> bool {
        matches!(
            language,
            "python"
                | "python3"
                | "python2"
                | "ruby"
                | "perl"
                | "node"
                | "javascript"
                | "js"
                | "lua"
                | "R"
                | "r"
        )
    }

    fn execute(
        &mut self,
        _block: &SrcBlock,
        _dir: &Path,
        _vars: &[(String, String)],
    ) -> ExecResult {
        // Script execution is handled by BabelExecutor::execute_shell() for
        // non-session blocks. Session blocks route through SessionManager.
        ExecResult::Error("ScriptBackend: use BabelExecutor::execute_shell()".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_script_languages() {
        let b = ScriptBackend;
        assert!(b.can_handle("python"));
        assert!(b.can_handle("python3"));
        assert!(b.can_handle("ruby"));
        assert!(b.can_handle("node"));
        assert!(b.can_handle("lua"));
        assert!(!b.can_handle("bash"));
        assert!(!b.can_handle("rust"));
    }
}
