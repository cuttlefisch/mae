//! Shell backend — subprocess per execution for shell languages.

use std::path::Path;

use super::LanguageBackend;
use crate::execute::ExecResult;
use crate::SrcBlock;

pub struct ShellBackend;

impl LanguageBackend for ShellBackend {
    fn name(&self) -> &str {
        "shell"
    }

    fn can_handle(&self, language: &str) -> bool {
        matches!(language, "bash" | "sh" | "zsh" | "fish")
    }

    fn execute(
        &mut self,
        _block: &SrcBlock,
        _dir: &Path,
        _vars: &[(String, String)],
    ) -> ExecResult {
        // Execution is handled by BabelExecutor::execute_shell() directly.
        // This backend exists for the trait registry pattern but delegates
        // to the existing subprocess code path.
        ExecResult::Error("ShellBackend: use BabelExecutor::execute_shell()".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_shell_languages() {
        let b = ShellBackend;
        assert!(b.can_handle("bash"));
        assert!(b.can_handle("sh"));
        assert!(b.can_handle("zsh"));
        assert!(b.can_handle("fish"));
        assert!(!b.can_handle("python"));
    }
}
