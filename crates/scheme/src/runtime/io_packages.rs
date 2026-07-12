//! File I/O, package infrastructure, module system functions, and
//! declarative package management (`mae!`/`package!`) primitives.
//!
//! Split out of `runtime.rs`'s `SchemeRuntime::new()` (CLAUDE.md architecture
//! debt reduction pass) — pure code motion, no behavior change.

use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use tracing::warn;

use crate::ffi::{arg_bool, arg_string, list_to_strings};
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

use super::{DeclaredPackage, SharedState};

/// Register file I/O, package infrastructure, module system, and
/// declarative package management (`mae!`/`package!`) primitives.
pub(super) fn register_io_package_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // --- File I/O ---

    // (read-file PATH)
    vm.register_fn(
        "read-file",
        "Read a file (capped at 1MB)",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let path = arg_string(args, 0, "read-file")?;
            match std::fs::read_to_string(&path) {
                Ok(content) if content.len() <= 1_048_576 => Ok(Value::string(content)),
                Ok(_) => Ok(Value::string("ERROR: file exceeds 1MB limit")),
                Err(e) => Ok(Value::string(format!("ERROR: {}", e))),
            }
        },
    );

    // (file-exists? PATH)
    vm.register_fn(
        "file-exists?",
        "Check if a file exists",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let path = arg_string(args, 0, "file-exists?")?;
            Ok(Value::Bool(std::path::Path::new(&path).exists()))
        },
    );

    // (list-directory PATH)
    vm.register_fn(
        "list-directory",
        "List directory entries",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let path = arg_string(args, 0, "list-directory")?;
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    let items: Vec<Value> = entries
                        .flatten()
                        .map(|e| {
                            let name = e.file_name().to_string_lossy().into_owned();
                            let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                            Value::list(vec![Value::string(name), Value::Bool(is_dir)])
                        })
                        .collect();
                    Ok(Value::list(items))
                }
                Err(_) => Ok(Value::Null),
            }
        },
    );

    // --- Package infrastructure ---

    // (provide-feature FEATURE)
    let s = shared.clone();
    vm.register_fn(
        "provide-feature",
        "Mark feature as loaded",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let feature = arg_string(args, 0, "provide-feature")?;
            s.lock().loaded_features.insert(feature);
            Ok(Value::Void)
        },
    );

    // (featurep FEATURE)
    let s = shared.clone();
    vm.register_fn(
        "featurep",
        "Check if feature is loaded",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let feature = arg_string(args, 0, "featurep")?;
            Ok(Value::Bool(s.lock().loaded_features.contains(&feature)))
        },
    );

    // (require-feature FEATURE)
    let s = shared.clone();
    vm.register_fn(
        "require-feature",
        "Request loading a feature",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let feature = arg_string(args, 0, "require-feature")?;
            let mut state = s.lock();
            if !state.loaded_features.contains(&feature) {
                state.pending_requires.push(feature);
            }
            Ok(Value::Void)
        },
    );

    // (load-path)
    let s = shared.clone();
    vm.register_fn(
        "load-path",
        "Return current load-path",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            let state = s.lock();
            let items: Vec<Value> = state
                .load_path
                .iter()
                .map(|p| Value::string(p.to_string_lossy().into_owned()))
                .collect();
            Ok(Value::list(items))
        },
    );

    // (add-to-load-path! DIR)
    let s = shared.clone();
    vm.register_fn(
        "add-to-load-path!",
        "Prepend directory to load-path",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let dir = arg_string(args, 0, "add-to-load-path!")?;
            s.lock().load_path.insert(0, PathBuf::from(dir));
            Ok(Value::Void)
        },
    );

    // (autoload COMMAND-NAME FEATURE DOC)
    let s = shared.clone();
    vm.register_fn(
        "autoload",
        "Register a command backed by autoload",
        Arity::Fixed(3),
        move |args: &[Value]| {
            let cmd_name = arg_string(args, 0, "autoload")?;
            let feature = arg_string(args, 1, "autoload")?;
            let doc = arg_string(args, 2, "autoload")?;
            s.lock().pending_autoloads.push((cmd_name, feature, doc));
            Ok(Value::Void)
        },
    );

    // --- Module system functions ---

    // (when-flag MODULE-NAME FLAG-NAME THUNK)
    if let Err(e) = vm.eval(
        r#"
(define (when-flag module-name flag-name thunk)
  (thunk))
"#,
    ) {
        warn!(error = %e, "scheme runtime: failed to define bootstrap `when-flag`");
    }

    // (define-option! NAME KIND DEFAULT DOC)
    let s = shared.clone();
    vm.register_fn(
        "define-option!",
        "Register a runtime option",
        Arity::Fixed(4),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "define-option!")?;
            let kind = arg_string(args, 1, "define-option!")?;
            let default = arg_string(args, 2, "define-option!")?;
            let doc = arg_string(args, 3, "define-option!")?;
            s.lock()
                .pending_dynamic_options
                .push((name, kind, default, doc));
            Ok(Value::Void)
        },
    );

    // (module-loaded? NAME)
    let s = shared.clone();
    vm.register_fn(
        "module-loaded?",
        "Check if a module is active",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "module-loaded?")?;
            Ok(Value::Bool(s.lock().active_modules.contains_key(&name)))
        },
    );

    // (module-version NAME)
    let s = shared.clone();
    vm.register_fn(
        "module-version",
        "Get version of active module",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "module-version")?;
            match s.lock().active_modules.get(&name) {
                Some(v) => Ok(Value::string(v.clone())),
                None => Ok(Value::Bool(false)),
            }
        },
    );

    // (module-list)
    let s = shared.clone();
    vm.register_fn(
        "module-list",
        "List all active module names",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            let state = s.lock();
            Ok(Value::list(
                state
                    .active_modules
                    .keys()
                    .map(|k| Value::string(k.clone()))
                    .collect::<Vec<_>>(),
            ))
        },
    );

    // (register-module! NAME VERSION)
    let s = shared.clone();
    vm.register_fn(
        "register-module!",
        "Register a loaded module",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "register-module!")?;
            let version = arg_string(args, 1, "register-module!")?;
            s.lock().active_modules.insert(name, version);
            Ok(Value::Void)
        },
    );

    // (when-module NAME THUNK) — Scheme-level wrapper
    if let Err(e) = vm.eval(
        r#"
(define (when-module name thunk)
  (when (module-loaded? name)
    (thunk)))
"#,
    ) {
        warn!(error = %e, "scheme runtime: failed to define bootstrap `when-module`");
    }

    // (module-flags NAME)
    vm.register_fn(
        "module-flags",
        "Get enabled flags for a module",
        Arity::Fixed(1),
        move |_args: &[Value]| Ok(Value::Null),
    );

    // --- Declarative package management (mae!, package!) ---

    // (mae-declare-module! NAME FLAGS)
    let s = shared.clone();
    vm.register_fn(
        "mae-declare-module!",
        "Declare a module with flags",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "mae-declare-module!")?;
            let flags = if args.len() > 1 {
                list_to_strings(&args[1])
            } else {
                vec![]
            };
            s.lock().declared_modules.insert(name, flags);
            Ok(Value::Void)
        },
    );

    // (mae-declared-modules)
    let s = shared.clone();
    vm.register_fn(
        "mae-declared-modules",
        "List declared module names",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            let state = s.lock();
            Ok(Value::list(
                state
                    .declared_modules
                    .keys()
                    .map(|k| Value::string(k.clone()))
                    .collect::<Vec<_>>(),
            ))
        },
    );

    // (mae-declare-package! NAME SOURCE PIN DISABLE)
    let s = shared.clone();
    vm.register_fn(
        "mae-declare-package!",
        "Declare a third-party package",
        Arity::Fixed(4),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "mae-declare-package!")?;
            let source = arg_string(args, 1, "mae-declare-package!")?;
            let pin = arg_string(args, 2, "mae-declare-package!")?;
            let disable = arg_bool(args, 3, "mae-declare-package!")?;
            s.lock().declared_packages.push(DeclaredPackage {
                name,
                source: if source.is_empty() {
                    None
                } else {
                    Some(source)
                },
                pin: if pin.is_empty() { None } else { Some(pin) },
                disable,
            });
            Ok(Value::Void)
        },
    );

    // Define mae! and package! Scheme-level wrappers
    if let Err(e) = vm.eval(
        r#"
;; Pre-define category labels
(define :editor ":editor")
(define :ui ":ui")
(define :lang ":lang")
(define :tools ":tools")
(define :completion ":completion")
(define :emacs ":emacs")
(define :term ":term")
(define :os ":os")
(define :app ":app")
(define :config ":config")
(define :input ":input")

(define (mae! . args)
  (for-each
    (lambda (item)
      (cond
        ((and (string? item)
              (> (string-length item) 0)
              (equal? (substring item 0 1) ":"))
         #f)
        ((list? item)
         (mae-declare-module! (car item) (cdr item)))
        ((string? item)
         (mae-declare-module! item '()))
        ((symbol? item)
         (mae-declare-module! (symbol->string item) '()))
        (else #f)))
    args))

(define :source ":source")
(define :pin ":pin")
(define :disable ":disable")

(define (package! name . kwargs)
  (define (kwarg-ref key default)
    (let loop ((rest kwargs))
      (cond
        ((null? rest) default)
        ((and (>= (length rest) 2)
              (equal? (car rest) key))
         (cadr rest))
        (else (loop (cdr rest))))))
  (mae-declare-package! name
                        (kwarg-ref ":source" "")
                        (kwarg-ref ":pin" "")
                        (if (kwarg-ref ":disable" #f) #t #f)))
"#,
    ) {
        warn!(error = %e, "scheme runtime: failed to define bootstrap `mae!`/`package!` wrappers");
    }

    // (undefine-command! NAME)
    let s = shared.clone();
    vm.register_fn(
        "undefine-command!",
        "Remove a command",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "undefine-command!")?;
            s.lock().pending_command_unregisters.push(name);
            Ok(Value::Void)
        },
    );

    // (undefine-option! NAME)
    let s = shared.clone();
    vm.register_fn(
        "undefine-option!",
        "Remove an option",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "undefine-option!")?;
            s.lock().pending_option_unregisters.push(name);
            Ok(Value::Void)
        },
    );

    // (unload-feature NAME)
    let s = shared.clone();
    vm.register_fn(
        "unload-feature",
        "Remove from loaded_features",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let name = arg_string(args, 0, "unload-feature")?;
            let removed = s.lock().loaded_features.remove(&name);
            Ok(Value::Bool(removed))
        },
    );

    // (define-kb-node! ID TITLE BODY)
    let s = shared.clone();
    vm.register_fn(
        "define-kb-node!",
        "Register a KB node from Scheme",
        Arity::Fixed(3),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "define-kb-node!")?;
            let title = arg_string(args, 1, "define-kb-node!")?;
            let body = arg_string(args, 2, "define-kb-node!")?;
            s.lock().pending_kb_nodes.push((id, title, body));
            Ok(Value::Void)
        },
    );

    // (kb-agenda FILTER [ARGS]) — dispatch graph agenda query
    let s = shared.clone();
    vm.register_fn(
        "kb-agenda",
        "Query KB graph: (kb-agenda \"orphan\"), (kb-agenda \"todo\" \"TODO\")",
        Arity::Variadic(1),
        move |args: &[Value]| {
            let filter = arg_string(args, 0, "kb-agenda")?;
            let extra = args.get(1).map(|v| format!("{}", v)).unwrap_or_default();
            let cmd = if extra.is_empty() {
                format!("kb-agenda {}", filter)
            } else {
                format!("kb-agenda {} {}", filter, extra)
            };
            s.lock().pending_ex_commands.push(cmd);
            Ok(Value::Void)
        },
    );

    // (kb-history NODE-ID) — show version history
    let s = shared.clone();
    vm.register_fn(
        "kb-history",
        "Show version history for a KB node",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-history")?;
            s.lock()
                .pending_ex_commands
                .push(format!("kb-history {}", id));
            Ok(Value::Void)
        },
    );

    // (kb-restore NODE-ID VERSION) — restore a node to a previous version
    let s = shared.clone();
    vm.register_fn(
        "kb-restore",
        "Restore a KB node to a previous version",
        Arity::Fixed(2),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-restore")?;
            let version = match &args[1] {
                Value::Int(n) => *n,
                other => return Err(LispError::type_error("integer", format!("{}", other))),
            };
            s.lock()
                .pending_ex_commands
                .push(format!("kb-restore {} {}", id, version));
            Ok(Value::Void)
        },
    );

    // (kb-raw-query DATALOG) — execute raw CozoDB Datalog query
    let s = shared.clone();
    vm.register_fn(
        "kb-raw-query",
        "Execute raw CozoDB Datalog query against the KB",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let query = arg_string(args, 0, "kb-raw-query")?;
            s.lock()
                .pending_ex_commands
                .push(format!("kb-raw-query {}", query));
            Ok(Value::Void)
        },
    );
}
