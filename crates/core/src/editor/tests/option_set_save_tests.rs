//! `:set-save` / `save_option_to_init` persistence tests.
//!
//! Extracted from `option_tests.rs` under CLAUDE.md's file-ceiling remedy — the
//! same treatment `babel_ops_tests.rs` documents for itself. `option_tests.rs`
//! crossed its ratchet tolerance when the principle-#16 config-save guard tests
//! were added (1440 -> 1590). This module was the natural seam: it is wholly
//! self-contained, owning `with_isolated_config_home` and `init_scm_contents`,
//! which nothing outside it uses.

use super::*;

// --- :set-save / save_option_to_init persistence ---
//
// save_option_to_init() does real filesystem I/O keyed off XDG_CONFIG_HOME,
// so tests must serialize (env vars are process-global) and use an isolated
// tmp dir — never a shared/well-known path (principle #14 test isolation).

mod set_save_tests {
    use super::*;

    /// Run `f` with XDG_CONFIG_HOME pointed at a fresh tmp dir, restoring
    /// the previous value afterwards even if `f` panics.
    fn with_isolated_config_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        // Recover from poisoning rather than propagating it. `f`'s panic is
        // caught below and re-raised *after* the env is restored, which
        // unwinds through this guard and poisons the mutex — so without this,
        // one genuinely-failing test turns every other test in the module into
        // a `PoisonError` and buries the real failure among a dozen fakes.
        // The data the lock guards is `()`; there is no invariant to protect.
        let _lock = mae_effect_sandbox::lock_env();
        let tmp = tempfile::tempdir().expect("tmpdir");
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        // The isolation above is precisely what licenses the opt-in: config
        // writes inside this closure land in `tmp`, not in the developer's
        // real `~/.config/mae`. See `crate::effect_sandbox`.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::effect_sandbox::with_external_effects(|| f(tmp.path()))
        }));
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        result.unwrap()
    }

    fn init_scm_contents(config_home: &std::path::Path) -> String {
        std::fs::read_to_string(config_home.join("mae").join("init.scm"))
            .expect("init.scm should exist")
    }

    #[test]
    fn creates_managed_section_when_init_scm_absent() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            let msg = editor.save_option_to_init("ai_chat_enabled").unwrap();
            assert!(msg.contains("ai_chat_enabled = true"));

            let content = init_scm_contents(config_home);
            assert!(content.contains(";; --- MAE managed options ---"));
            assert!(content.contains(";; --- end managed options ---"));
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    /// Principle #16's fourth path. CLAUDE.md names three surfaces that refuse
    /// to write `~/.config/mae/**` for an agent — `create_file`, `rename_file`,
    /// and AI-originated buffer saves — and `save_option_to_init` was none of
    /// them, while writing exactly that file. The `set_option` tool's
    /// `persist: true` flag, `(set-option-save! …)` through `eval_scheme`, and
    /// the `:set-save` command mirror all funnel here, so this is the one place
    /// the check belongs.
    ///
    /// The oracle is the **file on disk**, not the returned message: a refusal
    /// that still wrote init.scm would pass a message assertion, and init.scm
    /// is evaluated at full authority on the next launch.
    #[test]
    fn an_ai_originated_persist_cannot_write_init_scm() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();

            let result =
                editor.with_ai_dispatch_scope(|e| e.save_option_to_init("ai_chat_enabled"));

            assert!(
                result.is_err(),
                "an AI-originated :set-save wrote MAE's own config: {result:?}"
            );
            assert!(
                !config_home.join("mae").join("init.scm").exists(),
                "init.scm was created despite the refusal — the file is what matters, \
                 not the message"
            );
        });
    }

    /// The other half, so the fix above cannot be "satisfied" by breaking
    /// `:set-save` for everyone. A human's own persistence must still work —
    /// the asymmetry between human and AI *is* the control (principle #16).
    #[test]
    fn a_human_persist_still_writes_init_scm() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            assert!(init_scm_contents(config_home)
                .contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    /// An AI-originated persist that is refused must not leave a *previously
    /// written* init.scm modified either — the refusal has to come before any
    /// read-modify-write, not after a partial one.
    #[test]
    fn an_ai_originated_persist_leaves_an_existing_init_scm_untouched() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();
            let before = init_scm_contents(config_home);

            editor.set_option("ai_chat_enabled", "false").unwrap();
            let result =
                editor.with_ai_dispatch_scope(|e| e.save_option_to_init("ai_chat_enabled"));

            assert!(result.is_err(), "AI-originated persist was not refused");
            assert_eq!(
                init_scm_contents(config_home),
                before,
                "init.scm changed under a refused AI-originated persist"
            );
        });
    }

    #[test]
    fn appends_second_option_into_existing_managed_section() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            editor.set_option("spell_enabled", "true").unwrap();
            editor.save_option_to_init("spell_enabled").unwrap();

            let content = init_scm_contents(config_home);
            // Exactly one managed section, both options present inside it.
            assert_eq!(content.matches(";; --- MAE managed options ---").count(), 1);
            assert_eq!(content.matches(";; --- end managed options ---").count(), 1);
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
            assert!(content.contains("(set-option! \"spell_enabled\" \"true\")"));
        });
    }

    #[test]
    fn resaving_same_option_replaces_line_instead_of_duplicating() {
        // Adversarial: re-running :set-save for an option already present
        // must overwrite its line, not append a second, conflicting one —
        // a real Scheme file would apply both sequentially and "last write
        // wins" would be silently order-dependent instead of idempotent.
        with_isolated_config_home(|_config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            editor.set_option("ai_chat_enabled", "false").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            let content =
                std::fs::read_to_string(dirs_config_home_path().join("mae").join("init.scm"))
                    .unwrap();
            let occurrences = content
                .lines()
                .filter(|l| {
                    l.trim_start()
                        .starts_with("(set-option! \"ai_chat_enabled\"")
                })
                .count();
            assert_eq!(
                occurrences, 1,
                "resaving must replace the existing line, not duplicate it"
            );
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"false\")"));
            assert!(!content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    /// Resolve the XDG_CONFIG_HOME path the way save_option_to_init does,
    /// for tests that need to re-read the file after the closure captured
    /// `config_home` is out of scope.
    fn dirs_config_home_path() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap())
    }

    #[test]
    fn preserves_user_content_outside_managed_markers() {
        with_isolated_config_home(|config_home| {
            let mae_dir = config_home.join("mae");
            std::fs::create_dir_all(&mae_dir).unwrap();
            std::fs::write(
                mae_dir.join("init.scm"),
                "; my own config\n(define-key \"normal\" \"g g\" \"goto-first-line\")\n",
            )
            .unwrap();

            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            let content = init_scm_contents(config_home);
            assert!(content.contains("; my own config"));
            assert!(content.contains("(define-key \"normal\" \"g g\" \"goto-first-line\")"));
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    #[test]
    fn unknown_option_errors_without_touching_filesystem() {
        with_isolated_config_home(|config_home| {
            let editor = Editor::new();
            let result = editor.save_option_to_init("not_a_real_option");
            assert!(result.is_err());
            assert!(!config_home.join("mae").join("init.scm").exists());
        });
    }

    #[test]
    fn escapes_quotes_and_backslashes_in_string_values() {
        // Adversarial: a string-valued option (e.g. a shell command) may
        // legitimately contain a `"` or `\`. An unescaped write would emit
        // invalid Scheme (or worse, silently truncate the string at the
        // embedded quote), corrupting init.scm for every subsequent load.
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            let tricky = r#"echo "hi" \ done"#;
            editor.set_option("ai_api_key_command", tricky).unwrap();
            editor.save_option_to_init("ai_api_key_command").unwrap();

            let content = init_scm_contents(config_home);
            let expected = format!(
                "(set-option! \"ai_api_key_command\" \"{}\")",
                tricky.replace('\\', "\\\\").replace('"', "\\\"")
            );
            assert!(
                content.contains(&expected),
                "expected escaped line in init.scm, got:\n{}",
                content
            );

            // Round-trip: unescaping the written literal (mirroring R7RS
            // string-escape rules: \\ -> \, \" -> ") must reproduce the
            // original value exactly. mae-core has no dependency on the
            // Scheme reader itself, so this pins the same contract the
            // reader (crates/scheme/src/reader.rs) is expected to honor
            // without pulling that crate in as a test dependency.
            let escaped = tricky.replace('\\', "\\\\").replace('"', "\\\"");
            let mut unescaped = String::with_capacity(escaped.len());
            let mut chars = escaped.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some(next) => unescaped.push(next),
                        None => panic!("dangling escape in written literal"),
                    }
                } else {
                    unescaped.push(c);
                }
            }
            assert_eq!(unescaped, tricky, "escaping must be exactly reversible");
        });
    }

    /// Audit #599.1 — the branch predicate (`content.contains(pattern)`, a
    /// substring test) disagreed with the rewrite (`line.starts_with(pattern)`).
    /// So a COMMENTED-OUT or nested occurrence of the same `(set-option! "x"`
    /// text selected the replace branch, replaced nothing, and still reported
    /// "Saved" — the option silently never persisted. MAE's own shipped
    /// init.scm template is full of commented-out example lines, so this fired
    /// on the most ordinary config there is.
    #[test]
    fn a_commented_out_set_option_does_not_swallow_the_real_save() {
        // Several genuinely different shapes of "the pattern is present but
        // not as a settable line", not one hand-picked string.
        let decoys = [
            r#";; (set-option! "ai_chat_enabled" "false")"#,
            r#";  (set-option! "ai_chat_enabled" "false")"#,
            r#"; example: (set-option! "ai_chat_enabled" "false")"#,
            r#"(begin (set-option! "ai_chat_enabled" "false"))"#,
        ];

        for decoy in decoys {
            with_isolated_config_home(|config_home| {
                let init = config_home.join("mae").join("init.scm");
                std::fs::create_dir_all(init.parent().unwrap()).unwrap();
                std::fs::write(&init, format!("{decoy}\n")).unwrap();

                let mut editor = Editor::new();
                editor.set_option("ai_chat_enabled", "true").unwrap();
                let msg = editor.save_option_to_init("ai_chat_enabled").unwrap();
                assert!(msg.contains("Saved"), "{msg}");

                let content = init_scm_contents(config_home);
                // Selective oracle: a REAL, line-initial setter now exists with
                // the new value. Merely asserting the file changed would pass on
                // a write that only reformatted the decoy.
                assert!(
                    content
                        .lines()
                        .any(|l| l.trim_start() == r#"(set-option! "ai_chat_enabled" "true")"#),
                    "decoy {decoy:?} swallowed the save; init.scm is:\n{content}"
                );
                // And the user's own line is left exactly as they wrote it.
                assert!(
                    content.contains(decoy),
                    "the user's own line must not be rewritten:\n{content}"
                );
            });
        }
    }

    /// The counter-case: a genuine line-initial setter must still be REPLACED,
    /// not duplicated — the predicate change must not have turned every save
    /// into an append.
    #[test]
    fn a_real_line_initial_setter_is_still_replaced_not_appended() {
        with_isolated_config_home(|config_home| {
            let init = config_home.join("mae").join("init.scm");
            std::fs::create_dir_all(init.parent().unwrap()).unwrap();
            std::fs::write(&init, "(set-option! \"ai_chat_enabled\" \"false\")\n").unwrap();

            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            let content = init_scm_contents(config_home);
            assert_eq!(
                content.matches(r#"(set-option! "ai_chat_enabled""#).count(),
                1,
                "exactly one setter must remain:\n{content}"
            );
            assert!(content.contains(r#"(set-option! "ai_chat_enabled" "true")"#));
        });
    }

    #[test]
    fn set_save_command_applies_value_then_persists() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            assert!(!editor.ai_chat_enabled);

            editor.execute_command("set-save ai_chat_enabled true");

            assert!(
                editor.ai_chat_enabled,
                ":set-save must apply the value, not just persist it"
            );
            let content = init_scm_contents(config_home);
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    /// ADR-050 D4 / Phase H's explicit requirement: the live-sync option
    /// must be `:set-save`-able, default off.
    #[test]
    fn set_save_ai_guidance_export_live_sync_applies_value_then_persists() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            assert!(!editor.ai_guidance_export_live_sync, "must default to off");

            editor.execute_command("set-save ai_guidance_export_live_sync true");

            assert!(
                editor.ai_guidance_export_live_sync,
                ":set-save must apply the value, not just persist it"
            );
            let content = init_scm_contents(config_home);
            assert!(content.contains("(set-option! \"ai_guidance_export_live_sync\" \"true\")"));
        });
    }
}
