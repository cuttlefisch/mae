//! mae-scheme compiler: AST → bytecode.
//!
//! Compiles Scheme expressions into a linear bytecode sequence.
//! The compiler tracks tail position to emit TAIL_CALL for proper
//! tail calls (R7RS §3.5).
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use std::collections::HashMap;

use crate::lisp_error::{LispError, SourceLocation};
use crate::macros::{self, SyntaxRules};
use crate::value::{InternedSymbol, Value};

/// A single bytecode instruction.
#[derive(Clone, Debug)]
pub enum Op {
    /// Push a constant value onto the stack.
    Const(Value),
    /// Load a global variable by name.
    LoadGlobal(String),
    /// Store top of stack into a global variable.
    StoreGlobal(String),
    /// Define a global variable (like StoreGlobal but creates if absent).
    DefineGlobal(String),
    /// Load a local variable by stack offset from base pointer.
    LoadLocal(usize),
    /// Store into a local variable.
    StoreLocal(usize),
    /// Load an upvalue (captured variable from enclosing scope).
    LoadUpvalue(usize),
    /// Store into an upvalue.
    StoreUpvalue(usize),
    /// Call a function with N arguments. Stack: [fn, arg1, ..., argN]
    Call(usize),
    /// Tail call — reuse current frame. Same args as Call.
    TailCall(usize),
    /// Return from the current function.
    Return,
    /// Unconditional jump (relative offset from current IP).
    Jump(i32),
    /// Jump if top of stack is #f (pop the value).
    JumpIfFalse(i32),
    /// Pop top of stack.
    Pop,
    /// Duplicate top of stack.
    Dup,
    /// Create a closure from a CodeObject index + upvalue descriptors.
    MakeClosure(usize, Vec<UpvalueDesc>),
    /// Capture the current continuation (call/cc support).
    CaptureCc,
    /// Yield control to the host (async support).
    Yield,
    /// Apply function to argument list.
    Apply,
    /// Return multiple values.
    Values,
    /// Call with values (receive multiple values).
    CallWithValues,
    /// Push an exception handler. Jump offset is relative to next instruction.
    /// On exception, the handler is popped and execution jumps to the offset
    /// with the exception value on the stack.
    PushHandler(i32),
    /// Pop the current exception handler (normal exit from guarded body).
    PopHandler,
    /// Raise an exception (value on top of stack).
    Raise,
    /// No-op / placeholder.
    Nop,
}

/// Describes how to capture an upvalue when creating a closure.
#[derive(Clone, Debug)]
pub enum UpvalueDesc {
    /// Capture from the enclosing function's locals.
    Local(usize),
    /// Capture from the enclosing function's upvalues (transitive).
    Upvalue(usize),
}

/// A compiled function/code object.
#[derive(Clone, Debug)]
pub struct CodeObject {
    /// The bytecode instructions.
    pub ops: Vec<Op>,
    /// Number of required parameters.
    pub arity: usize,
    /// Whether this function accepts rest args.
    pub variadic: bool,
    /// Function name (for debugging).
    pub name: Option<String>,
    /// Source location for debugging.
    pub source: Option<SourceLocation>,
    /// Source map: instruction index → source location.
    pub source_map: Vec<Option<SourceLocation>>,
}

impl CodeObject {
    fn new() -> Self {
        CodeObject {
            ops: Vec::new(),
            arity: 0,
            variadic: false,
            name: None,
            source: None,
            source_map: Vec::new(),
        }
    }

    fn emit(&mut self, op: Op) {
        self.source_map.push(None);
        self.ops.push(op);
    }

    #[allow(dead_code)]
    fn emit_at(&mut self, op: Op, loc: Option<SourceLocation>) {
        self.source_map.push(loc);
        self.ops.push(op);
    }

    fn current_offset(&self) -> usize {
        self.ops.len()
    }

    /// Patch a Jump or JumpIfFalse at `index` to jump to `target`.
    fn patch_jump(&mut self, index: usize, target: usize) {
        let offset = target as i32 - index as i32 - 1;
        match &mut self.ops[index] {
            Op::Jump(ref mut o) => *o = offset,
            Op::JumpIfFalse(ref mut o) => *o = offset,
            Op::PushHandler(ref mut o) => *o = offset,
            _ => panic!("patch_jump on non-jump instruction"),
        }
    }
}

/// Tracks local variables in the current scope during compilation.
#[derive(Clone, Debug)]
struct Local {
    name: String,
    #[allow(dead_code)]
    depth: usize,
}

/// Compiler state for a single function scope.
struct CompileScope {
    code: CodeObject,
    locals: Vec<Local>,
    upvalues: Vec<UpvalueDesc>,
    scope_depth: usize,
}

impl CompileScope {
    fn new() -> Self {
        CompileScope {
            code: CodeObject::new(),
            locals: Vec::new(),
            upvalues: Vec::new(),
            scope_depth: 0,
        }
    }

    fn resolve_local(&self, name: &str) -> Option<usize> {
        for (i, local) in self.locals.iter().enumerate().rev() {
            if local.name == name {
                return Some(i);
            }
        }
        None
    }

    fn add_local(&mut self, name: String) -> usize {
        let idx = self.locals.len();
        self.locals.push(Local {
            name,
            depth: self.scope_depth,
        });
        idx
    }

    fn add_upvalue(&mut self, desc: UpvalueDesc) -> usize {
        // Check if we already captured this upvalue
        for (i, existing) in self.upvalues.iter().enumerate() {
            match (existing, &desc) {
                (UpvalueDesc::Local(a), UpvalueDesc::Local(b)) if a == b => return i,
                (UpvalueDesc::Upvalue(a), UpvalueDesc::Upvalue(b)) if a == b => return i,
                _ => {}
            }
        }
        let idx = self.upvalues.len();
        self.upvalues.push(desc);
        idx
    }
}

/// A macro definition (either define-macro or syntax-rules).
#[derive(Clone, Debug)]
pub enum MacroDef {
    /// `(define-macro (name params...) body)` — template-based.
    /// Stores (param-names, body-template).
    Template { params: Vec<String>, body: Value },
    /// `(define-syntax name (syntax-rules ...))` — hygienic.
    SyntaxRules(SyntaxRules),
}

/// The compiler: transforms Value AST into bytecode CodeObjects.
pub struct Compiler {
    /// Pool of compiled code objects (functions).
    pub code_pool: Vec<CodeObject>,
    /// Stack of compilation scopes (for nested functions).
    scopes: Vec<CompileScope>,
    /// Macro definitions (populated during compilation).
    pub macros: HashMap<String, MacroDef>,
    /// Search paths for `include` and `load` (R7RS §4.1.7).
    pub load_paths: Vec<std::path::PathBuf>,
}

impl Compiler {
    pub fn new() -> Self {
        Compiler {
            code_pool: Vec::new(),
            scopes: vec![CompileScope::new()],
            macros: HashMap::new(),
            load_paths: Vec::new(),
        }
    }

    /// Compile a top-level expression. Returns the index of the code object.
    pub fn compile_top_level(&mut self, exprs: &[Value]) -> Result<usize, LispError> {
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            self.compile_expr(expr, is_last)?;
            if !is_last {
                self.emit(Op::Pop);
            }
        }
        if exprs.is_empty() {
            self.emit(Op::Const(Value::Void));
        }
        self.emit(Op::Return);

        let scope = self.scopes.pop().unwrap();
        let idx = self.code_pool.len();
        self.code_pool.push(scope.code);
        Ok(idx)
    }

    /// Compile a single expression.
    /// `tail` indicates whether this is in tail position.
    fn compile_expr(&mut self, expr: &Value, tail: bool) -> Result<(), LispError> {
        match expr {
            // Self-evaluating: numbers, strings, booleans, chars, vectors, bytevectors
            Value::Int(_)
            | Value::Float(_)
            | Value::Bool(_)
            | Value::Char(_)
            | Value::String(_)
            | Value::Void
            | Value::Null => {
                self.emit(Op::Const(expr.clone()));
                Ok(())
            }

            // Symbol → variable reference
            Value::Symbol(sym) => {
                self.compile_variable_ref(sym);
                Ok(())
            }

            // List → function call or special form
            Value::Pair(_) => {
                let items = expr.to_vec().map_err(|_| {
                    LispError::syntax("improper list in expression", format!("{expr}"))
                })?;

                if items.is_empty() {
                    return Err(LispError::syntax("empty application", "()"));
                }

                // Check for special forms
                if let Value::Symbol(sym) = &items[0] {
                    match sym.name() {
                        "quote" => return self.compile_quote(&items),
                        "if" => return self.compile_if(&items, tail),
                        "lambda" => return self.compile_lambda(&items),
                        "define" => return self.compile_define(&items),
                        "set!" => return self.compile_set(&items),
                        "begin" => return self.compile_begin(&items[1..], tail),
                        "let" => return self.compile_let(&items, tail),
                        "let*" => return self.compile_let_star(&items, tail),
                        "letrec" | "letrec*" => return self.compile_letrec(&items, tail),
                        "and" => return self.compile_and(&items[1..], tail),
                        "or" => return self.compile_or(&items[1..], tail),
                        "cond" => return self.compile_cond(&items[1..], tail),
                        "when" => return self.compile_when(&items, tail),
                        "unless" => return self.compile_unless(&items, tail),
                        "define-values" => return self.compile_define_values(&items),
                        "define-record-type" => return self.compile_define_record_type(&items),
                        "define-macro" => return self.compile_define_macro(&items),
                        "define-syntax" => return self.compile_define_syntax(&items),
                        "guard" => return self.compile_guard(&items, tail),
                        "raise" => return self.compile_raise(&items),
                        "with-exception-handler" => {
                            return self.compile_with_exception_handler(&items, tail)
                        }
                        "quasiquote" => return self.compile_quasiquote(&items),
                        "case" => return self.compile_case(&items, tail),
                        "case-lambda" => return self.compile_case_lambda(&items),
                        "do" => return self.compile_do(&items, tail),
                        "parameterize" => return self.compile_parameterize(&items, tail),
                        "let-values" => return self.compile_let_values(&items, tail),
                        "let*-values" => return self.compile_let_star_values(&items, tail),
                        "receive" => return self.compile_receive(&items, tail),
                        "apply" => return self.compile_apply(&items, tail),
                        "call-with-current-continuation" | "call/cc" => {
                            return self.compile_call_cc(&items, tail)
                        }
                        "cond-expand" => return self.compile_cond_expand(&items, tail),
                        "syntax-error" => return self.compile_syntax_error(&items),
                        "let-syntax" | "letrec-syntax" => {
                            return self.compile_let_syntax(&items, tail)
                        }
                        "include" => return self.compile_include(&items, tail, false),
                        "include-ci" => return self.compile_include(&items, tail, true),
                        name => {
                            // Check for macro expansion
                            if let Some(mac) = self.macros.get(name).cloned() {
                                let expanded = self.expand_macro(&mac, &items)?;
                                return self.compile_expr(&expanded, tail);
                            }
                        }
                    }
                }

                // Regular function call
                self.compile_call(&items, tail)
            }

            // Vectors as literals
            Value::Vector(_) => {
                self.emit(Op::Const(expr.clone()));
                Ok(())
            }

            Value::Bytevector(_) => {
                self.emit(Op::Const(expr.clone()));
                Ok(())
            }

            _ => Err(LispError::syntax("cannot compile", format!("{expr}"))),
        }
    }

    // -----------------------------------------------------------------------
    // Variable references
    // -----------------------------------------------------------------------

    fn compile_variable_ref(&mut self, sym: &InternedSymbol) {
        let name = sym.name();

        // Check locals in current scope
        if let Some(idx) = self.current_scope().resolve_local(name) {
            self.emit(Op::LoadLocal(idx));
            return;
        }

        // Check upvalues (captured from enclosing scopes)
        if self.scopes.len() > 1 {
            if let Some(idx) = self.resolve_upvalue(self.scopes.len() - 1, name) {
                self.emit(Op::LoadUpvalue(idx));
                return;
            }
        }

        // Global variable
        self.emit(Op::LoadGlobal(name.to_string()));
    }

    fn resolve_upvalue(&mut self, scope_idx: usize, name: &str) -> Option<usize> {
        if scope_idx == 0 {
            return None;
        }

        // Check locals in the parent scope
        let parent_idx = scope_idx - 1;
        if let Some(local_idx) = self.scopes[parent_idx].resolve_local(name) {
            let upvalue_idx = self.scopes[scope_idx].add_upvalue(UpvalueDesc::Local(local_idx));
            return Some(upvalue_idx);
        }

        // Check parent's upvalues (transitive capture)
        if let Some(parent_upvalue) = self.resolve_upvalue(parent_idx, name) {
            let upvalue_idx =
                self.scopes[scope_idx].add_upvalue(UpvalueDesc::Upvalue(parent_upvalue));
            return Some(upvalue_idx);
        }

        None
    }

    // -----------------------------------------------------------------------
    // Special forms
    // -----------------------------------------------------------------------

    fn compile_quote(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() != 2 {
            return Err(LispError::syntax("quote requires exactly 1 argument", ""));
        }
        self.emit(Op::Const(items[1].clone()));
        Ok(())
    }

    fn compile_if(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 || items.len() > 4 {
            return Err(LispError::syntax("if requires 2 or 3 arguments", ""));
        }

        // Compile condition
        self.compile_expr(&items[1], false)?;

        // Jump to else if false
        let else_jump = self.emit_placeholder(Op::JumpIfFalse(0));

        // Compile consequent (in tail position if if is in tail position)
        self.compile_expr(&items[2], tail)?;

        if items.len() == 4 {
            // Jump over else branch
            let end_jump = self.emit_placeholder(Op::Jump(0));

            // Patch else jump to here
            let else_target = self.current_offset();
            self.patch_jump(else_jump, else_target);

            // Compile alternative
            self.compile_expr(&items[3], tail)?;

            // Patch end jump to here
            let end_target = self.current_offset();
            self.patch_jump(end_jump, end_target);
        } else {
            // No else: result is void
            let end_jump = self.emit_placeholder(Op::Jump(0));

            let else_target = self.current_offset();
            self.patch_jump(else_jump, else_target);

            self.emit(Op::Const(Value::Void));

            let end_target = self.current_offset();
            self.patch_jump(end_jump, end_target);
        }

        Ok(())
    }

    fn compile_lambda(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("lambda requires formals and body", ""));
        }

        // Parse formals
        let (params, variadic) = self.parse_formals(&items[1])?;

        // Push new scope
        let mut scope = CompileScope::new();
        scope.code.arity = params.len();
        scope.code.variadic = variadic;

        // For variadic, the last param is the rest arg — arity is params.len()-1
        if variadic && !params.is_empty() {
            scope.code.arity = params.len() - 1;
        }

        self.scopes.push(scope);

        // Add parameters as locals
        for param in &params {
            self.current_scope_mut().add_local(param.clone());
        }

        // Compile body (last expression in tail position)
        let body = &items[2..];
        self.compile_begin(body, true)?;
        self.emit(Op::Return);

        // Pop scope and create code object
        let scope = self.scopes.pop().unwrap();
        let upvalues = scope.upvalues.clone();
        let code_idx = self.code_pool.len();
        self.code_pool.push(scope.code);

        // Emit closure creation in the enclosing scope
        self.emit(Op::MakeClosure(code_idx, upvalues));

        Ok(())
    }

    fn parse_formals(&self, formals: &Value) -> Result<(Vec<String>, bool), LispError> {
        match formals {
            // (lambda (a b c) ...) — fixed arity
            Value::Pair(_) | Value::Null => {
                let mut params = Vec::new();
                let mut current = formals.clone();
                loop {
                    match current {
                        Value::Null => return Ok((params, false)),
                        Value::Pair(p) => {
                            let name = p
                                .0
                                .as_symbol()
                                .map_err(|_| {
                                    LispError::syntax("formal must be a symbol", format!("{}", p.0))
                                })?
                                .name()
                                .to_string();
                            params.push(name);
                            current = p.1.clone();
                        }
                        // Dotted pair: rest parameter
                        Value::Symbol(sym) => {
                            params.push(sym.name().to_string());
                            return Ok((params, true));
                        }
                        _ => {
                            return Err(LispError::syntax(
                                "invalid formal parameter",
                                format!("{current}"),
                            ))
                        }
                    }
                }
            }
            // (lambda args ...) — single rest parameter
            Value::Symbol(sym) => Ok((vec![sym.name().to_string()], true)),
            _ => Err(LispError::syntax("invalid formals", format!("{formals}"))),
        }
    }

    fn compile_define(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax(
                "define requires at least 2 arguments",
                "",
            ));
        }

        match &items[1] {
            // (define x expr)
            Value::Symbol(sym) => {
                if items.len() != 3 {
                    return Err(LispError::syntax(
                        "define with symbol requires exactly 1 value",
                        "",
                    ));
                }
                let name = sym.name().to_string();
                self.compile_expr(&items[2], false)?;

                if self.scopes.len() == 1 {
                    // Top-level define
                    self.emit(Op::DefineGlobal(name));
                } else {
                    // Local define (internal definition)
                    let idx = self.current_scope_mut().add_local(name);
                    self.emit(Op::StoreLocal(idx));
                }
                self.emit(Op::Const(Value::Void));
            }
            // (define (f args...) body...) → (define f (lambda (args...) body...))
            Value::Pair(p) => {
                let name =
                    p.0.as_symbol()
                        .map_err(|_| LispError::syntax("define name must be a symbol", ""))?
                        .name()
                        .to_string();

                // Build lambda from formals and body
                let formals = p.1.clone();
                let mut lambda_items = vec![Value::symbol("lambda"), formals];
                lambda_items.extend_from_slice(&items[2..]);

                self.compile_lambda(&lambda_items)?;

                // Set the name on the closure's code object
                if let Some(code) = self.code_pool.last_mut() {
                    code.name = Some(name.clone());
                }

                if self.scopes.len() == 1 {
                    self.emit(Op::DefineGlobal(name));
                } else {
                    let idx = self.current_scope_mut().add_local(name);
                    self.emit(Op::StoreLocal(idx));
                }
                self.emit(Op::Const(Value::Void));
            }
            _ => {
                return Err(LispError::syntax(
                    "invalid define form",
                    format!("{}", items[1]),
                ))
            }
        }

        Ok(())
    }

    fn compile_set(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() != 3 {
            return Err(LispError::syntax("set! requires exactly 2 arguments", ""));
        }

        let sym = items[1]
            .as_symbol()
            .map_err(|_| LispError::syntax("set! target must be a symbol", ""))?;
        let name = sym.name();

        self.compile_expr(&items[2], false)?;

        // Check locals
        if let Some(idx) = self.current_scope().resolve_local(name) {
            self.emit(Op::StoreLocal(idx));
        } else if self.scopes.len() > 1 {
            if let Some(idx) = self.resolve_upvalue(self.scopes.len() - 1, name) {
                self.emit(Op::StoreUpvalue(idx));
            } else {
                self.emit(Op::StoreGlobal(name.to_string()));
            }
        } else {
            self.emit(Op::StoreGlobal(name.to_string()));
        }

        self.emit(Op::Const(Value::Void));
        Ok(())
    }

    fn compile_begin(&mut self, exprs: &[Value], tail: bool) -> Result<(), LispError> {
        if exprs.is_empty() {
            self.emit(Op::Const(Value::Void));
            return Ok(());
        }
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            self.compile_expr(expr, tail && is_last)?;
            if !is_last {
                self.emit(Op::Pop);
            }
        }
        Ok(())
    }

    fn compile_let(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        // (let ((x 1) (y 2)) body...)
        // Desugars to: ((lambda (x y) body...) 1 2)
        // This is the R7RS §4.2.2 definition, and ensures locals get their own frame.
        if items.len() < 3 {
            return Err(LispError::syntax("let requires bindings and body", ""));
        }

        // Named let: (let name ((x 1)) body...) → recursive
        if let Value::Symbol(loop_name) = &items[1] {
            return self.compile_named_let(loop_name.name(), &items[2..], tail);
        }

        let bindings = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("let bindings must be a list", ""))?;

        let mut params = Vec::new();
        let mut init_exprs = Vec::new();

        for binding in &bindings {
            let pair = binding
                .to_vec()
                .map_err(|_| LispError::syntax("let binding must be (var expr)", ""))?;
            if pair.len() != 2 {
                return Err(LispError::syntax("let binding must be (var expr)", ""));
            }
            let name = pair[0]
                .as_symbol()
                .map_err(|_| LispError::syntax("let variable must be a symbol", ""))?
                .name()
                .to_string();
            params.push(name);
            init_exprs.push(pair[1].clone());
        }

        // Build: ((lambda (params...) body...) init-exprs...)
        let formals = Value::list(params.iter().map(|p| Value::symbol(p)));
        let mut lambda_items = vec![Value::symbol("lambda"), formals];
        lambda_items.extend_from_slice(&items[2..]);
        let lambda = Value::list(lambda_items);

        // Compile the lambda (the function)
        self.compile_expr(&lambda, false)?;

        // Compile the init expressions (the arguments)
        for init in &init_exprs {
            self.compile_expr(init, false)?;
        }

        // Call the lambda with the arguments
        if tail {
            self.emit(Op::TailCall(init_exprs.len()));
        } else {
            self.emit(Op::Call(init_exprs.len()));
        }

        Ok(())
    }

    fn compile_named_let(
        &mut self,
        name: &str,
        items: &[Value],
        tail: bool,
    ) -> Result<(), LispError> {
        if items.len() < 2 {
            return Err(LispError::syntax(
                "named let requires bindings and body",
                "",
            ));
        }

        let bindings = items[0]
            .to_vec()
            .map_err(|_| LispError::syntax("let bindings must be a list", ""))?;

        // Extract param names and init values
        let mut params = Vec::new();
        let mut inits = Vec::new();
        for binding in &bindings {
            let pair = binding
                .to_vec()
                .map_err(|_| LispError::syntax("let binding must be (var expr)", ""))?;
            if pair.len() != 2 {
                return Err(LispError::syntax("let binding must be (var expr)", ""));
            }
            params.push(
                pair[0]
                    .as_symbol()
                    .map_err(|_| LispError::syntax("let variable must be a symbol", ""))?
                    .name()
                    .to_string(),
            );
            inits.push(pair[1].clone());
        }

        // Build: (letrec ((name (lambda (params...) body...))) (name inits...))
        let formals = Value::list(params.iter().map(|p| Value::symbol(p)));
        let mut lambda_items = vec![Value::symbol("lambda"), formals];
        lambda_items.extend_from_slice(&items[1..]);
        let lambda = Value::list(lambda_items);

        let binding = Value::list(vec![Value::symbol(name), lambda]);
        let binding_list = Value::list(vec![binding]);

        let mut call = vec![Value::symbol(name)];
        call.extend(inits);
        let call_expr = Value::list(call);

        let letrec = Value::list(vec![Value::symbol("letrec"), binding_list, call_expr]);

        let items_vec = letrec.to_vec().unwrap();
        self.compile_letrec(&items_vec, tail)
    }

    fn compile_let_star(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        // (let* ((x 1) (y 2)) body...)
        // Desugars to nested lets: (let ((x 1)) (let ((y 2)) body...))
        if items.len() < 3 {
            return Err(LispError::syntax("let* requires bindings and body", ""));
        }

        let bindings = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("let* bindings must be a list", ""))?;

        if bindings.is_empty() {
            // No bindings — just compile the body
            return self.compile_begin(&items[2..], tail);
        }

        // Build nested let from inside out
        let body: Vec<Value> = items[2..].to_vec();
        let mut result = {
            let mut inner = vec![
                Value::symbol("let"),
                Value::list(vec![bindings.last().unwrap().clone()]),
            ];
            inner.extend(body);
            Value::list(inner)
        };

        for binding in bindings[..bindings.len() - 1].iter().rev() {
            result = Value::list(vec![
                Value::symbol("let"),
                Value::list(vec![binding.clone()]),
                result,
            ]);
        }

        let items_vec = result.to_vec().unwrap();
        self.compile_let(&items_vec, tail)
    }

    fn compile_letrec(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("letrec requires bindings and body", ""));
        }

        let bindings = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("letrec bindings must be a list", ""))?;

        // letrec uses globals as a simple workaround for the mutable-cell
        // upvalue problem (closures capture values, not references).
        // This works because letrec semantics guarantee the names are
        // only referenced after all inits complete.
        let mut names = Vec::new();
        let mut init_exprs = Vec::new();
        for binding in &bindings {
            let pair = binding
                .to_vec()
                .map_err(|_| LispError::syntax("letrec binding must be (var expr)", ""))?;
            if pair.len() != 2 {
                return Err(LispError::syntax("letrec binding must be (var expr)", ""));
            }
            let name = pair[0]
                .as_symbol()
                .map_err(|_| LispError::syntax("letrec variable must be a symbol", ""))?
                .name()
                .to_string();
            names.push(name);
            init_exprs.push(pair[1].clone());
        }

        // Define all names as globals with undefined
        for name in &names {
            self.emit(Op::Const(Value::Undefined));
            self.emit(Op::DefineGlobal(name.clone()));
        }

        // Evaluate init expressions and assign to globals
        for (name, init) in names.iter().zip(init_exprs.iter()) {
            self.compile_expr(init, false)?;
            self.emit(Op::StoreGlobal(name.clone()));
        }

        self.compile_begin(&items[2..], tail)?;

        Ok(())
    }

    fn compile_and(&mut self, exprs: &[Value], tail: bool) -> Result<(), LispError> {
        if exprs.is_empty() {
            self.emit(Op::Const(Value::Bool(true)));
            return Ok(());
        }

        let mut end_jumps = Vec::new();
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            self.compile_expr(expr, is_last && tail)?;
            if !is_last {
                self.emit(Op::Dup);
                let jump = self.emit_placeholder(Op::JumpIfFalse(0));
                self.emit(Op::Pop); // pop the dup'd value (it was truthy)
                end_jumps.push(jump);
            }
        }
        // Patch: if any was false, jump to the end with that false value
        let end = self.current_offset();
        for jump in end_jumps {
            self.patch_jump(jump, end);
        }
        Ok(())
    }

    fn compile_or(&mut self, exprs: &[Value], tail: bool) -> Result<(), LispError> {
        if exprs.is_empty() {
            self.emit(Op::Const(Value::Bool(false)));
            return Ok(());
        }

        let mut end_jumps = Vec::new();
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            self.compile_expr(expr, is_last && tail)?;
            if !is_last {
                self.emit(Op::Dup);
                // Jump to end if true (skip remaining)
                let not_true_jump = self.emit_placeholder(Op::JumpIfFalse(0));
                let true_jump = self.emit_placeholder(Op::Jump(0));
                let after_false = self.current_offset();
                self.patch_jump(not_true_jump, after_false);
                self.emit(Op::Pop); // pop the dup'd false value
                end_jumps.push(true_jump);
            }
        }
        let end = self.current_offset();
        for jump in end_jumps {
            self.patch_jump(jump, end);
        }
        Ok(())
    }

    fn compile_cond(&mut self, clauses: &[Value], tail: bool) -> Result<(), LispError> {
        if clauses.is_empty() {
            self.emit(Op::Const(Value::Void));
            return Ok(());
        }

        let mut end_jumps = Vec::new();

        for clause in clauses {
            let items = clause
                .to_vec()
                .map_err(|_| LispError::syntax("cond clause must be a list", ""))?;
            if items.is_empty() {
                return Err(LispError::syntax("empty cond clause", ""));
            }

            // (else body...)
            if let Value::Symbol(sym) = &items[0] {
                if sym.name() == "else" {
                    self.compile_begin(&items[1..], tail)?;
                    let end = self.current_offset();
                    for jump in end_jumps {
                        self.patch_jump(jump, end);
                    }
                    return Ok(());
                }
            }

            // (test body...)
            self.compile_expr(&items[0], false)?;
            let skip_jump = self.emit_placeholder(Op::JumpIfFalse(0));

            if items.len() > 1 {
                self.compile_begin(&items[1..], tail)?;
            }
            // If no body, the test result is the value (already on stack...
            // but JumpIfFalse pops it). We need to re-evaluate. For now,
            // cond clauses without body return void.

            end_jumps.push(self.emit_placeholder(Op::Jump(0)));

            let skip_target = self.current_offset();
            self.patch_jump(skip_jump, skip_target);
        }

        // No else clause matched: return void
        self.emit(Op::Const(Value::Void));
        let end = self.current_offset();
        for jump in end_jumps {
            self.patch_jump(jump, end);
        }

        Ok(())
    }

    fn compile_when(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("when requires test and body", ""));
        }
        // (when test body...) → (if test (begin body...) (void))
        self.compile_expr(&items[1], false)?;
        let skip = self.emit_placeholder(Op::JumpIfFalse(0));
        self.compile_begin(&items[2..], tail)?;
        let end = self.emit_placeholder(Op::Jump(0));
        let skip_target = self.current_offset();
        self.patch_jump(skip, skip_target);
        self.emit(Op::Const(Value::Void));
        let end_target = self.current_offset();
        self.patch_jump(end, end_target);
        Ok(())
    }

    fn compile_unless(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("unless requires test and body", ""));
        }
        // (unless test body...) → (if (not test) (begin body...) (void))
        self.compile_expr(&items[1], false)?;
        let skip = self.emit_placeholder(Op::JumpIfFalse(0));
        self.emit(Op::Const(Value::Void));
        let end = self.emit_placeholder(Op::Jump(0));
        let skip_target = self.current_offset();
        self.patch_jump(skip, skip_target);
        self.compile_begin(&items[2..], tail)?;
        let end_target = self.current_offset();
        self.patch_jump(end, end_target);
        Ok(())
    }

    fn compile_define_values(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() != 3 {
            return Err(LispError::syntax(
                "define-values requires formals and expr",
                "",
            ));
        }
        let formals = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("define-values formals must be a list", ""))?;

        if formals.len() == 1 {
            // Simple case: (define-values (x) expr) → (define x expr)
            let name = formals[0]
                .as_symbol()
                .map_err(|_| LispError::syntax("define-values formal must be a symbol", ""))?
                .name()
                .to_string();
            self.compile_expr(&items[2], false)?;
            self.emit(Op::DefineGlobal(name));
            self.emit(Op::Const(Value::Void));
            Ok(())
        } else {
            // Multi-variable: (define-values (x y z) expr)
            // Desugar to:
            //   (begin
            //     (define __dv_tmp (call-with-values (lambda () expr) list))
            //     (define x (list-ref __dv_tmp 0))
            //     (define y (list-ref __dv_tmp 1))
            //     (define z (list-ref __dv_tmp 2)))
            let tmp = "__dv_tmp";
            let expr = items[2].clone();

            // Build: (call-with-values (lambda () expr) list)
            let cwv = Value::list(vec![
                Value::symbol("call-with-values"),
                Value::list(vec![Value::symbol("lambda"), Value::Null, expr]),
                Value::symbol("list"),
            ]);

            // Compile: (define __dv_tmp <cwv>)
            self.compile_expr(&cwv, false)?;
            self.emit(Op::DefineGlobal(tmp.to_string()));

            // For each formal, compile: (define <name> (list-ref __dv_tmp <i>))
            for (i, formal) in formals.iter().enumerate() {
                let name = formal
                    .as_symbol()
                    .map_err(|_| LispError::syntax("define-values formal must be a symbol", ""))?
                    .name()
                    .to_string();
                let list_ref_expr = Value::list(vec![
                    Value::symbol("list-ref"),
                    Value::symbol(tmp),
                    Value::Int(i as i64),
                ]);
                self.compile_expr(&list_ref_expr, false)?;
                self.emit(Op::DefineGlobal(name));
            }

            self.emit(Op::Const(Value::Void));
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // let-values / let*-values / receive (R7RS §4.2.2, SRFI-8)
    // -----------------------------------------------------------------------

    /// Compile `(let-values (((x y) expr) ...) body ...)`
    /// Desugars to: `(let ((temp expr)) (let ((x (list-ref temp 0)) (y (list-ref temp 1))) body))`
    /// For single-binding case, simplifies to call-with-values pattern.
    fn compile_let_values(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        // (let-values ((formals expr) ...) body ...)
        if items.len() < 3 {
            return Err(LispError::syntax(
                "let-values requires bindings and body",
                "",
            ));
        }
        let bindings = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("let-values bindings must be a list", ""))?;
        let body = &items[2..];

        // Build nested lets for each binding clause
        let mut result = Value::list(
            std::iter::once(Value::symbol("begin"))
                .chain(body.iter().cloned())
                .collect::<Vec<_>>(),
        );

        // Process bindings in reverse order (innermost first)
        for binding in bindings.iter().rev() {
            let clause = binding
                .to_vec()
                .map_err(|_| LispError::syntax("let-values clause must be a list", ""))?;
            if clause.len() != 2 {
                return Err(LispError::syntax(
                    "let-values clause needs (formals expr)",
                    "",
                ));
            }
            let formals = clause[0]
                .to_vec()
                .map_err(|_| LispError::syntax("let-values formals must be a list", ""))?;
            let expr = &clause[1];

            // Desugar to: (call-with-values (lambda () expr) (lambda (formals) body))
            let consumer_lambda =
                Value::list(vec![Value::symbol("lambda"), Value::list(formals), result]);
            let producer_lambda = Value::list(vec![
                Value::symbol("lambda"),
                Value::list(vec![]),
                expr.clone(),
            ]);
            result = Value::list(vec![
                Value::symbol("call-with-values"),
                producer_lambda,
                consumer_lambda,
            ]);
        }

        self.compile_expr(&result, tail)
    }

    /// Compile `(let*-values ...)` — sequential version of let-values.
    /// Same as let-values since each binding is visible to subsequent ones.
    fn compile_let_star_values(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        // let*-values has the same semantics as let-values when each binding
        // introduces independent variables (which they do in our desugaring)
        self.compile_let_values(items, tail)
    }

    /// Compile `(let-syntax ((name transformer) ...) body ...)` and
    /// `(letrec-syntax ...)` — local macro definitions.
    /// Both forms bind macros for the duration of the body.
    fn compile_let_syntax(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax(
                "let-syntax requires bindings and body",
                "",
            ));
        }
        let bindings = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("let-syntax bindings must be a list", ""))?;

        // Save current macros, add local ones, compile body, restore
        let saved_macros = self.macros.clone();

        for binding in &bindings {
            let clause = binding
                .to_vec()
                .map_err(|_| LispError::syntax("let-syntax clause must be a list", ""))?;
            if clause.len() != 2 {
                return Err(LispError::syntax(
                    "let-syntax clause needs (name transformer)",
                    "",
                ));
            }
            let name = clause[0]
                .as_symbol()
                .map_err(|_| LispError::syntax("let-syntax name must be a symbol", ""))?
                .name()
                .to_string();
            // Process the transformer (syntax-rules form)
            let sr_items = clause[1].to_vec().map_err(|_| {
                LispError::syntax("let-syntax transformer must be a syntax-rules form", "")
            })?;
            if sr_items.is_empty() {
                return Err(LispError::syntax("let-syntax: empty transformer", ""));
            }
            match &sr_items[0] {
                Value::Symbol(s) if s.name() == "syntax-rules" => {
                    let rules = macros::parse_syntax_rules(&sr_items)?;
                    self.macros.insert(name, MacroDef::SyntaxRules(rules));
                }
                _ => {
                    return Err(LispError::syntax(
                        "let-syntax: only syntax-rules supported",
                        "",
                    ))
                }
            }
        }

        // Compile body as begin
        let body = &items[2..];
        self.compile_begin(body, tail)?;

        // Restore macros
        self.macros = saved_macros;
        Ok(())
    }

    /// Compile `(receive formals expr body ...)` (SRFI-8).
    /// Desugars to: `(call-with-values (lambda () expr) (lambda formals body ...))`
    fn compile_receive(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        // (receive formals expr body ...)
        if items.len() < 4 {
            return Err(LispError::syntax(
                "receive requires formals, expr, and body",
                "",
            ));
        }
        let formals = items[1].clone();
        let expr = &items[2];
        let body = &items[3..];

        let producer = Value::list(vec![
            Value::symbol("lambda"),
            Value::list(vec![]),
            expr.clone(),
        ]);
        let mut consumer_items = vec![Value::symbol("lambda"), formals];
        consumer_items.extend_from_slice(body);
        let consumer = Value::list(consumer_items);

        let desugared = Value::list(vec![Value::symbol("call-with-values"), producer, consumer]);
        self.compile_expr(&desugared, tail)
    }

    // -----------------------------------------------------------------------
    // cond-expand (R7RS §4.2.1) + syntax-error (R7RS §4.3.1)
    // -----------------------------------------------------------------------

    /// Compile `(cond-expand (feature-req body ...) ... (else body ...))`.
    /// Feature-based conditional expansion at compile time.
    fn compile_cond_expand(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        let features = vec!["r7rs", "mae", "ratios", "exact-complex"];

        for clause in &items[1..] {
            let parts = clause
                .to_vec()
                .map_err(|_| LispError::syntax("cond-expand clause must be a list", ""))?;
            if parts.is_empty() {
                continue;
            }

            // Check if this clause matches
            if self.cond_expand_matches(&parts[0], &features)? {
                // Compile the body expressions
                return self.compile_begin(&parts[1..], tail);
            }
        }

        // No clause matched — R7RS says this is an error
        Err(LispError::syntax("cond-expand: no matching clause", ""))
    }

    fn cond_expand_matches(&self, req: &Value, features: &[&str]) -> Result<bool, LispError> {
        match req {
            Value::Symbol(sym) if sym.name() == "else" => Ok(true),
            Value::Symbol(sym) => Ok(features.contains(&sym.name())),
            Value::Pair(_) => {
                let parts = req.to_vec().map_err(|_| {
                    LispError::syntax("cond-expand requirement must be symbol or list", "")
                })?;
                if parts.is_empty() {
                    return Ok(false);
                }
                match parts[0].as_symbol().map(|s| s.name().to_string()) {
                    Ok(ref name) if name == "and" => {
                        for part in &parts[1..] {
                            if !self.cond_expand_matches(part, features)? {
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    }
                    Ok(ref name) if name == "or" => {
                        for part in &parts[1..] {
                            if self.cond_expand_matches(part, features)? {
                                return Ok(true);
                            }
                        }
                        Ok(false)
                    }
                    Ok(ref name) if name == "not" => {
                        if parts.len() != 2 {
                            return Err(LispError::syntax("cond-expand not requires 1 arg", ""));
                        }
                        Ok(!self.cond_expand_matches(&parts[1], features)?)
                    }
                    Ok(ref name) if name == "library" => {
                        // (library (scheme base)) — check if library is available
                        // For now, we support the standard R7RS libraries
                        if parts.len() != 2 {
                            return Ok(false);
                        }
                        let lib_name = format!("{}", parts[1]);
                        Ok(matches!(
                            lib_name.as_str(),
                            "(scheme base)"
                                | "(scheme case-lambda)"
                                | "(scheme char)"
                                | "(scheme cxr)"
                                | "(scheme eval)"
                                | "(scheme inexact)"
                                | "(scheme lazy)"
                                | "(scheme read)"
                                | "(scheme write)"
                                | "(mae base)"
                        ))
                    }
                    _ => Ok(false),
                }
            }
            _ => Ok(false),
        }
    }

    /// Compile `(include "file1" "file2" ...)` — read and splice file contents.
    /// `include-ci` folds the source to lowercase before reading.
    fn compile_include(
        &mut self,
        items: &[Value],
        tail: bool,
        case_insensitive: bool,
    ) -> Result<(), LispError> {
        if items.len() < 2 {
            return Err(LispError::syntax(
                "include requires at least one filename",
                "",
            ));
        }
        let mut all_exprs = Vec::new();
        for item in &items[1..] {
            let filename = item
                .as_str()
                .map_err(|_| LispError::syntax("include: filename must be a string", ""))?;

            // Search load paths
            let mut found = None;
            let path = std::path::Path::new(filename);
            if path.is_absolute() && path.exists() {
                found = Some(path.to_path_buf());
            } else {
                for dir in &self.load_paths {
                    let candidate = dir.join(filename);
                    if candidate.exists() {
                        found = Some(candidate);
                        break;
                    }
                }
                // Also try relative to CWD
                if found.is_none() && path.exists() {
                    found = Some(path.to_path_buf());
                }
            }

            let resolved = found.ok_or_else(|| {
                LispError::syntax(format!("include: file not found: {filename}"), "")
            })?;

            let mut source = std::fs::read_to_string(&resolved).map_err(|e| {
                LispError::syntax(
                    format!("include: error reading {}: {e}", resolved.display()),
                    "",
                )
            })?;

            if case_insensitive {
                source = source.to_lowercase();
            }

            let datums = crate::reader::read_all(&source)?;
            all_exprs.extend(datums);
        }

        if all_exprs.is_empty() {
            self.emit(Op::Const(Value::Void));
        } else {
            self.compile_begin(&all_exprs, tail)?;
        }
        Ok(())
    }

    /// Compile `(syntax-error message irritant ...)` — compile-time error.
    fn compile_syntax_error(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() < 2 {
            return Err(LispError::syntax("syntax-error requires a message", ""));
        }
        let msg = match &items[1] {
            Value::String(s) => s.to_string(),
            other => format!("{other}"),
        };
        Err(LispError::syntax(&msg, ""))
    }

    // -----------------------------------------------------------------------
    // define-record-type (R7RS §5.5)
    // -----------------------------------------------------------------------

    /// Compile `(define-record-type <name> (ctor field ...) pred (field accessor [mutator]) ...)`.
    /// Desugars to a begin block with define for constructor, predicate, and accessors.
    fn compile_define_record_type(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() < 4 {
            return Err(LispError::syntax(
                "define-record-type requires type-name, constructor, predicate, and fields",
                "",
            ));
        }

        let type_name = items[1]
            .as_symbol()
            .map_err(|_| LispError::syntax("record type name must be a symbol", ""))?
            .name()
            .to_string();

        let ctor_parts = items[2]
            .to_list()
            .ok_or_else(|| LispError::syntax("constructor must be a list", ""))?;
        if ctor_parts.is_empty() {
            return Err(LispError::syntax("constructor needs a name", ""));
        }
        let ctor_name = ctor_parts[0]
            .as_symbol()
            .map_err(|_| LispError::syntax("constructor name must be a symbol", ""))?
            .name()
            .to_string();
        let ctor_fields: Vec<String> = ctor_parts[1..]
            .iter()
            .map(|v| {
                v.as_symbol()
                    .map(|s| s.name().to_string())
                    .map_err(|_| LispError::syntax("constructor field must be a symbol", ""))
            })
            .collect::<Result<_, _>>()?;

        let pred_name = items[3]
            .as_symbol()
            .map_err(|_| LispError::syntax("predicate name must be a symbol", ""))?
            .name()
            .to_string();

        let field_specs = &items[4..];

        // Build the desugared code as a begin block
        let mut defs = Vec::new();

        // Constructor: (define (ctor f1 f2 ...) (vector 'type-name f1 f2 ...))
        let formals = Value::list(ctor_fields.iter().map(|f| Value::symbol(f)));
        let mut vec_args = vec![
            Value::symbol("vector"),
            Value::list(vec![Value::symbol("quote"), Value::symbol(&type_name)]),
        ];
        vec_args.extend(ctor_fields.iter().map(|f| Value::symbol(f)));
        let ctor_body = Value::list(vec_args);
        defs.push(Value::list(vec![
            Value::symbol("define"),
            Value::cons(Value::symbol(&ctor_name), formals),
            ctor_body,
        ]));

        // Predicate: (define (pred obj) (and (vector? obj) (> (vector-length obj) 0) (eq? (vector-ref obj 0) 'type-name)))
        let pred_body = Value::list(vec![
            Value::symbol("and"),
            Value::list(vec![Value::symbol("vector?"), Value::symbol("__rec_obj__")]),
            Value::list(vec![
                Value::symbol(">"),
                Value::list(vec![
                    Value::symbol("vector-length"),
                    Value::symbol("__rec_obj__"),
                ]),
                Value::Int(0),
            ]),
            Value::list(vec![
                Value::symbol("eq?"),
                Value::list(vec![
                    Value::symbol("vector-ref"),
                    Value::symbol("__rec_obj__"),
                    Value::Int(0),
                ]),
                Value::list(vec![Value::symbol("quote"), Value::symbol(&type_name)]),
            ]),
        ]);
        defs.push(Value::list(vec![
            Value::symbol("define"),
            Value::list(vec![
                Value::symbol(&pred_name),
                Value::symbol("__rec_obj__"),
            ]),
            pred_body,
        ]));

        // Field accessors and mutators
        for (i, spec) in field_specs.iter().enumerate() {
            let parts = spec
                .to_list()
                .ok_or_else(|| LispError::syntax("field spec must be a list", ""))?;
            if parts.len() < 2 {
                return Err(LispError::syntax(
                    "field spec needs at least (name accessor)",
                    "",
                ));
            }

            let idx = (i + 1) as i64; // field 0 is the type tag

            // Accessor: (define (accessor obj) (vector-ref obj idx))
            let accessor_name = parts[1]
                .as_symbol()
                .map_err(|_| LispError::syntax("accessor must be a symbol", ""))?
                .name()
                .to_string();
            defs.push(Value::list(vec![
                Value::symbol("define"),
                Value::list(vec![
                    Value::symbol(&accessor_name),
                    Value::symbol("__rec_obj__"),
                ]),
                Value::list(vec![
                    Value::symbol("vector-ref"),
                    Value::symbol("__rec_obj__"),
                    Value::Int(idx),
                ]),
            ]));

            // Mutator (optional): (define (mutator obj val) (vector-set! obj idx val))
            if parts.len() >= 3 {
                let mutator_name = parts[2]
                    .as_symbol()
                    .map_err(|_| LispError::syntax("mutator must be a symbol", ""))?
                    .name()
                    .to_string();
                defs.push(Value::list(vec![
                    Value::symbol("define"),
                    Value::list(vec![
                        Value::symbol(&mutator_name),
                        Value::symbol("__rec_obj__"),
                        Value::symbol("__rec_val__"),
                    ]),
                    Value::list(vec![
                        Value::symbol("vector-set!"),
                        Value::symbol("__rec_obj__"),
                        Value::Int(idx),
                        Value::symbol("__rec_val__"),
                    ]),
                ]));
            }
        }

        // Compile as (begin def1 def2 ...)
        self.compile_begin(&defs, false)
    }

    // -----------------------------------------------------------------------
    // Function calls
    // -----------------------------------------------------------------------

    fn compile_call(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        // Compile the function expression
        self.compile_expr(&items[0], false)?;

        // Compile arguments
        let argc = items.len() - 1;
        for arg in &items[1..] {
            self.compile_expr(arg, false)?;
        }

        // Emit call (tail call if in tail position)
        if tail && self.scopes.len() > 1 {
            self.emit(Op::TailCall(argc));
        } else {
            self.emit(Op::Call(argc));
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Macros
    // -----------------------------------------------------------------------

    /// Compile `(define-macro (name params...) body)`.
    /// Stores the macro definition and emits Void.
    fn compile_define_macro(&mut self, items: &[Value]) -> Result<(), LispError> {
        // (define-macro (name param...) body)
        if items.len() < 3 {
            return Err(LispError::syntax("define-macro requires name and body", ""));
        }
        let sig = items[1].to_vec().map_err(|_| {
            LispError::syntax(
                "define-macro: expected (name params...)",
                format!("{}", items[1]),
            )
        })?;
        if sig.is_empty() {
            return Err(LispError::syntax("define-macro: empty signature", ""));
        }
        let name = match &sig[0] {
            Value::Symbol(s) => s.name().to_string(),
            _ => {
                return Err(LispError::syntax(
                    "define-macro: name must be symbol",
                    format!("{}", sig[0]),
                ))
            }
        };
        let params: Vec<String> = sig[1..]
            .iter()
            .map(|v| match v {
                Value::Symbol(s) => Ok(s.name().to_string()),
                _ => Err(LispError::syntax(
                    "define-macro: param must be symbol",
                    format!("{v}"),
                )),
            })
            .collect::<Result<_, _>>()?;

        // For multiple body expressions, wrap in begin
        let body = if items.len() == 3 {
            items[2].clone()
        } else {
            let mut begin = vec![Value::symbol("begin")];
            begin.extend_from_slice(&items[2..]);
            Value::list(begin)
        };

        self.macros
            .insert(name, MacroDef::Template { params, body });
        self.emit(Op::Const(Value::Void));
        Ok(())
    }

    /// Compile `(define-syntax name (syntax-rules ...))`.
    fn compile_define_syntax(&mut self, items: &[Value]) -> Result<(), LispError> {
        // (define-syntax name transformer)
        if items.len() != 3 {
            return Err(LispError::syntax(
                "define-syntax requires name and transformer",
                "",
            ));
        }
        let name = match &items[1] {
            Value::Symbol(s) => s.name().to_string(),
            _ => {
                return Err(LispError::syntax(
                    "define-syntax: name must be symbol",
                    format!("{}", items[1]),
                ))
            }
        };
        let transformer_items = items[2].to_vec().map_err(|_| {
            LispError::syntax(
                "define-syntax: expected (syntax-rules ...)",
                format!("{}", items[2]),
            )
        })?;
        if transformer_items.is_empty() {
            return Err(LispError::syntax("define-syntax: empty transformer", ""));
        }
        match &transformer_items[0] {
            Value::Symbol(s) if s.name() == "syntax-rules" => {
                let rules = macros::parse_syntax_rules(&transformer_items)?;
                self.macros.insert(name, MacroDef::SyntaxRules(rules));
            }
            _ => {
                return Err(LispError::syntax(
                    "define-syntax: only syntax-rules supported",
                    format!("{}", items[2]),
                ))
            }
        }
        self.emit(Op::Const(Value::Void));
        Ok(())
    }

    /// Expand a macro application.
    fn expand_macro(&self, mac: &MacroDef, items: &[Value]) -> Result<Value, LispError> {
        match mac {
            MacroDef::Template { params, body } => {
                // define-macro: body is evaluated with params bound to produce expansion.
                // We build a mini-VM to evaluate the body.
                let args = &items[1..];
                if args.len() != params.len() {
                    return Err(LispError::syntax(
                        format!("macro expects {} args, got {}", params.len(), args.len()),
                        format!("{}", Value::list(items.to_vec())),
                    ));
                }
                // Build (let ((p1 (quote a1)) (p2 (quote a2)) ...) body)
                let bindings_list: Vec<Value> = params
                    .iter()
                    .zip(args.iter())
                    .map(|(p, a)| {
                        Value::list(vec![
                            Value::symbol(p),
                            Value::list(vec![Value::symbol("quote"), a.clone()]),
                        ])
                    })
                    .collect();
                let let_expr = Value::list(vec![
                    Value::symbol("let"),
                    Value::list(bindings_list),
                    body.clone(),
                ]);
                // Evaluate using a temporary VM with stdlib
                let mut vm = crate::vm::Vm::new();
                crate::stdlib::register_stdlib(&mut vm);
                vm.eval(&format!("{let_expr}"))
            }
            MacroDef::SyntaxRules(rules) => macros::expand_syntax_rules(rules, items),
        }
    }

    // -----------------------------------------------------------------------
    // Quasiquote
    // -----------------------------------------------------------------------

    /// Compile `(quasiquote template)` — R7RS §4.2.8.
    /// Expands quasiquote as a syntax transformation, then compiles the result.
    /// This follows Chibi-Scheme's approach: quasiquote → cons/append tree.
    fn compile_quasiquote(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() != 2 {
            return Err(LispError::syntax(
                "quasiquote requires exactly 1 argument",
                "",
            ));
        }
        let expanded = Self::expand_qq(&items[1], 0)?;
        self.compile_expr(&expanded, false)
    }

    /// Expand quasiquote template into cons/append/quote expressions.
    /// Follows the Chibi-Scheme expansion algorithm:
    /// - `(unquote x)` at depth 0 → x
    /// - `(unquote-splicing x)` in car at depth 0 → (append x (expand cdr))
    /// - Regular pair → (cons (expand car) (expand cdr))
    /// - Atom → (quote atom)
    fn expand_qq(template: &Value, depth: usize) -> Result<Value, LispError> {
        match template {
            Value::Pair(p) => {
                // Check for (unquote expr) — the WHOLE form is (unquote expr)
                if let Value::Symbol(s) = &p.0 {
                    if s.name() == "unquote" {
                        if let Some(items) = p.1.to_list() {
                            if items.len() == 1 {
                                if depth == 0 {
                                    return Ok(items[0].clone());
                                }
                                let inner = Self::expand_qq(&items[0], depth - 1)?;
                                return Ok(Value::list(vec![
                                    Value::symbol("list"),
                                    Value::list(vec![
                                        Value::symbol("quote"),
                                        Value::symbol("unquote"),
                                    ]),
                                    inner,
                                ]));
                            }
                        }
                        return Err(LispError::syntax("bad unquote", ""));
                    }
                    if s.name() == "quasiquote" {
                        if let Some(items) = p.1.to_list() {
                            if items.len() == 1 {
                                let inner = Self::expand_qq(&items[0], depth + 1)?;
                                return Ok(Value::list(vec![
                                    Value::symbol("list"),
                                    Value::list(vec![
                                        Value::symbol("quote"),
                                        Value::symbol("quasiquote"),
                                    ]),
                                    inner,
                                ]));
                            }
                        }
                    }
                }

                // Check car for (unquote-splicing expr)
                if let Value::Pair(car_pair) = &p.0 {
                    if let Value::Symbol(s) = &car_pair.0 {
                        if s.name() == "unquote-splicing" && depth == 0 {
                            if let Some(splice_args) = car_pair.1.to_list() {
                                if splice_args.len() == 1 {
                                    let cdr_expanded = Self::expand_qq(&p.1, depth)?;
                                    return Ok(Value::list(vec![
                                        Value::symbol("append"),
                                        splice_args[0].clone(),
                                        cdr_expanded,
                                    ]));
                                }
                            }
                        }
                    }
                }

                // Regular pair: (cons (expand car) (expand cdr))
                // This handles the case where car is (unquote x) as an element:
                // expand_qq on (unquote x) will match the Symbol("unquote") check above
                // and return x directly, so (cons x (expand cdr)) is correct.
                let car_exp = Self::expand_qq(&p.0, depth)?;
                let cdr_exp = Self::expand_qq(&p.1, depth)?;
                Ok(Value::list(vec![Value::symbol("cons"), car_exp, cdr_exp]))
            }
            // Atoms are self-quoting
            _ => Ok(Value::list(vec![Value::symbol("quote"), template.clone()])),
        }
    }

    // -----------------------------------------------------------------------
    // Case expression
    // -----------------------------------------------------------------------

    /// Compile `(case expr clause ...)` — R7RS §4.2.1.
    /// Desugars to `(let ((key expr)) (cond ...))` with `eqv?` tests.
    fn compile_case(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("case requires expr and clauses", ""));
        }

        let key_sym = Value::symbol("__case_key__");

        // Build cond clauses from case clauses
        let mut cond_clauses = Vec::new();
        for clause in &items[2..] {
            let parts = clause
                .to_list()
                .ok_or_else(|| LispError::syntax("case clause must be a list", ""))?;
            if parts.is_empty() {
                return Err(LispError::syntax("empty case clause", ""));
            }

            if let Value::Symbol(s) = &parts[0] {
                if s.name() == "else" {
                    cond_clauses.push(clause.clone());
                    break;
                }
            }

            // ((datum ...) body...) → ((or (eqv? key 'd1) (eqv? key 'd2) ...) body...)
            let datums = parts[0]
                .to_list()
                .ok_or_else(|| LispError::syntax("case datums must be a list", ""))?;

            let test = if datums.len() == 1 {
                Value::list(vec![
                    Value::symbol("eqv?"),
                    key_sym.clone(),
                    Value::list(vec![Value::symbol("quote"), datums[0].clone()]),
                ])
            } else {
                let mut or_parts = vec![Value::symbol("or")];
                for datum in &datums {
                    or_parts.push(Value::list(vec![
                        Value::symbol("eqv?"),
                        key_sym.clone(),
                        Value::list(vec![Value::symbol("quote"), datum.clone()]),
                    ]));
                }
                Value::list(or_parts)
            };

            let mut cond_clause = vec![test];
            cond_clause.extend(parts[1..].iter().cloned());
            cond_clauses.push(Value::list(cond_clause));
        }

        let mut cond_expr_parts = vec![Value::symbol("cond")];
        cond_expr_parts.extend(cond_clauses);
        let cond_expr = Value::list(cond_expr_parts);

        // (let ((key expr)) (cond ...))
        let let_expr = Value::list(vec![
            Value::symbol("let"),
            Value::list(vec![Value::list(vec![key_sym, items[1].clone()])]),
            cond_expr,
        ]);

        let items_vec = let_expr.to_vec().unwrap();
        self.compile_let(&items_vec, tail)
    }

    // -----------------------------------------------------------------------
    // Case-lambda
    // -----------------------------------------------------------------------

    /// Compile `(case-lambda clause ...)` — R7RS §4.2.9.
    /// Each clause is ((formals ...) body ...).
    /// Desugars to a lambda that dispatches on argument count.
    fn compile_case_lambda(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() < 2 {
            return Err(LispError::syntax(
                "case-lambda requires at least one clause",
                "",
            ));
        }

        // Parse all clauses to determine max arity
        let mut clauses = Vec::new();
        for clause in &items[1..] {
            let parts = clause
                .to_list()
                .ok_or_else(|| LispError::syntax("case-lambda clause must be a list", ""))?;
            if parts.is_empty() {
                return Err(LispError::syntax("case-lambda clause needs formals", ""));
            }
            let (params, variadic) = self.parse_formals(&parts[0])?;
            clauses.push((params, variadic, parts[1..].to_vec()));
        }

        // Build a single variadic lambda that dispatches on (length args)
        // (lambda args
        //   (let ((n (length args)))
        //     (cond
        //       ((= n arity1) (apply (lambda (formals1) body1) args))
        //       ((= n arity2) (apply (lambda (formals2) body2) args))
        //       ...)))
        let args_sym = Value::symbol("__cl_args__");
        let n_sym = Value::symbol("__cl_n__");

        let mut cond_clauses = Vec::new();
        for (params, variadic, body) in &clauses {
            // Build the inner lambda
            let formals = if *variadic && params.len() > 1 {
                // (x y . rest) — dotted pair
                let mut pairs = params.iter().map(|p| Value::symbol(p)).collect::<Vec<_>>();
                let rest = pairs.pop().unwrap();
                let mut result = rest;
                for p in pairs.into_iter().rev() {
                    result = Value::Pair(std::rc::Rc::new((p, result)));
                }
                result
            } else if *variadic {
                Value::symbol(&params[0])
            } else {
                Value::list(params.iter().map(|p| Value::symbol(p)))
            };

            let mut lambda_parts = vec![Value::symbol("lambda"), formals];
            lambda_parts.extend(body.iter().cloned());
            let lambda = Value::list(lambda_parts);

            let required = if *variadic {
                params.len().saturating_sub(1)
            } else {
                params.len()
            };

            // Test: (= n required) for fixed, (>= n required) for variadic
            let test = if *variadic {
                Value::list(vec![
                    Value::symbol(">="),
                    n_sym.clone(),
                    Value::Int(required as i64),
                ])
            } else {
                Value::list(vec![
                    Value::symbol("="),
                    n_sym.clone(),
                    Value::Int(required as i64),
                ])
            };

            // Body: (apply lambda args)
            let apply_expr = Value::list(vec![Value::symbol("apply"), lambda, args_sym.clone()]);

            cond_clauses.push(Value::list(vec![test, apply_expr]));
        }

        // Add error clause
        cond_clauses.push(Value::list(vec![
            Value::symbol("else"),
            Value::list(vec![
                Value::symbol("error"),
                Value::string("case-lambda: no matching clause"),
            ]),
        ]));

        // (lambda args (let ((n (length args))) (cond ...)))
        let length_call = Value::list(vec![Value::symbol("length"), args_sym.clone()]);
        let n_binding = Value::list(vec![Value::list(vec![n_sym.clone(), length_call])]);
        let cond_expr = {
            let mut parts = vec![Value::symbol("cond")];
            parts.extend(cond_clauses);
            Value::list(parts)
        };
        let let_expr = Value::list(vec![Value::symbol("let"), n_binding, cond_expr]);
        let full_lambda = Value::list(vec![
            Value::symbol("lambda"),
            Value::symbol("__cl_args__"),
            let_expr,
        ]);

        let items_vec = full_lambda.to_vec().unwrap();
        self.compile_lambda(&items_vec)
    }

    // -----------------------------------------------------------------------
    // Do iteration
    // -----------------------------------------------------------------------

    /// Compile `(do ((var init step) ...) (test expr ...) body ...)` — R7RS §4.2.4.
    /// Desugars to named let.
    fn compile_do(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax(
                "do requires vars, test, and optionally body",
                "",
            ));
        }

        let var_specs = items[1]
            .to_list()
            .ok_or_else(|| LispError::syntax("do variable specs must be a list", ""))?;

        let test_clause = items[2]
            .to_list()
            .ok_or_else(|| LispError::syntax("do test clause must be a list", ""))?;

        if test_clause.is_empty() {
            return Err(LispError::syntax("do test clause is empty", ""));
        }

        let body = &items[3..];

        // Parse variable specifications: (var init [step])
        let mut var_names = Vec::new();
        let mut init_exprs = Vec::new();
        let mut step_exprs = Vec::new();

        for spec in &var_specs {
            let parts = spec
                .to_list()
                .ok_or_else(|| LispError::syntax("do var spec must be a list", ""))?;
            if parts.len() < 2 || parts.len() > 3 {
                return Err(LispError::syntax(
                    "do var spec must be (var init) or (var init step)",
                    "",
                ));
            }
            let name = parts[0]
                .as_symbol()
                .map_err(|_| LispError::syntax("do var must be a symbol", ""))?
                .name()
                .to_string();
            var_names.push(name.clone());
            init_exprs.push(parts[1].clone());
            if parts.len() == 3 {
                step_exprs.push(parts[2].clone());
            } else {
                step_exprs.push(Value::symbol(&name)); // no step = keep current
            }
        }

        // Desugar to named let:
        // (let __do_loop__ ((var1 init1) (var2 init2) ...)
        //   (if test
        //     (begin expr ...)
        //     (begin body ... (__do_loop__ step1 step2 ...))))
        let loop_name = "__do_loop__";
        let bindings = Value::list(
            var_names
                .iter()
                .zip(init_exprs.iter())
                .map(|(name, init)| Value::list(vec![Value::symbol(name), init.clone()])),
        );

        let test = &test_clause[0];
        let result_exprs = if test_clause.len() > 1 {
            &test_clause[1..]
        } else {
            &[Value::Void][..]
        };

        // Build step call: (__do_loop__ step1 step2 ...)
        let mut step_call = vec![Value::symbol(loop_name)];
        step_call.extend(step_exprs.iter().cloned());
        let step = Value::list(step_call);

        // Build loop body: body... then recurse
        let mut loop_body = Vec::new();
        loop_body.extend(body.iter().cloned());
        loop_body.push(step);
        let else_branch = if loop_body.len() == 1 {
            loop_body[0].clone()
        } else {
            let mut begin = vec![Value::symbol("begin")];
            begin.extend(loop_body);
            Value::list(begin)
        };

        let result_branch = if result_exprs.len() == 1 {
            result_exprs[0].clone()
        } else {
            let mut begin = vec![Value::symbol("begin")];
            begin.extend(result_exprs.iter().cloned());
            Value::list(begin)
        };

        let if_expr = Value::list(vec![
            Value::symbol("if"),
            test.clone(),
            result_branch,
            else_branch,
        ]);

        let named_let = Value::list(vec![
            Value::symbol("let"),
            Value::symbol(loop_name),
            bindings,
            if_expr,
        ]);

        let items_vec = named_let.to_vec().unwrap();
        self.compile_let(&items_vec, tail)
    }

    // -----------------------------------------------------------------------
    // Parameterize
    // -----------------------------------------------------------------------

    /// Compile `(parameterize ((param val) ...) body ...)` — R7RS §4.2.6.
    /// Desugars to dynamic-wind + parameter mutation.
    fn compile_parameterize(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax(
                "parameterize requires bindings and body",
                "",
            ));
        }

        let bindings = items[1]
            .to_list()
            .ok_or_else(|| LispError::syntax("parameterize bindings must be a list", ""))?;

        // Desugar to:
        // (let ((saved1 (param1)) (saved2 (param2)) ...)
        //   (dynamic-wind
        //     (lambda () (param1 val1) (param2 val2) ...)
        //     (lambda () body ...)
        //     (lambda () (param1 saved1) (param2 saved2) ...)))
        //
        // But since dynamic-wind may not be available yet, we can also use
        // a simpler approach: save, set, body, restore.
        // For now, use the simpler approach since it doesn't need dynamic-wind.

        let mut save_bindings = Vec::new();
        let mut set_before = Vec::new();
        let mut set_after = Vec::new();

        for (i, binding) in bindings.iter().enumerate() {
            let parts = binding
                .to_list()
                .ok_or_else(|| LispError::syntax("parameterize binding must be a list", ""))?;
            if parts.len() != 2 {
                return Err(LispError::syntax(
                    "parameterize binding must be (param val)",
                    "",
                ));
            }
            let param = &parts[0];
            let val = &parts[1];
            let saved_name = format!("__param_saved_{i}__");

            // saved = (param) — call param with no args to get current value
            save_bindings.push(Value::list(vec![
                Value::symbol(&saved_name),
                Value::list(vec![param.clone()]),
            ]));

            // (param val) — set new value
            set_before.push(Value::list(vec![param.clone(), val.clone()]));

            // (param saved) — restore old value
            set_after.push(Value::list(vec![param.clone(), Value::symbol(&saved_name)]));
        }

        // Build: (let ((saved1 (p1)) ...) (p1 v1) ... (let ((result (begin body))) (p1 saved1) ... result))
        let save_list = Value::list(save_bindings);

        let mut body_parts = set_before;

        // Result binding
        let result_sym = Value::symbol("__param_result__");
        let body_expr = if items[2..].len() == 1 {
            items[2].clone()
        } else {
            let mut begin = vec![Value::symbol("begin")];
            begin.extend(items[2..].iter().cloned());
            Value::list(begin)
        };

        let result_binding = Value::list(vec![Value::list(vec![result_sym.clone(), body_expr])]);

        let mut inner_body = set_after;
        inner_body.push(result_sym);

        let mut inner_let_parts = vec![Value::symbol("let"), result_binding];
        inner_let_parts.extend(inner_body);
        let inner_let = Value::list(inner_let_parts);

        body_parts.push(inner_let);

        let mut outer_parts = vec![Value::symbol("let"), save_list];
        outer_parts.extend(body_parts);
        let outer = Value::list(outer_parts);

        let items_vec = outer.to_vec().unwrap();
        self.compile_let(&items_vec, tail)
    }

    /// Compile `(apply fn arg ... list)`.
    fn compile_apply(&mut self, items: &[Value], _tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("apply requires at least 2 arguments", ""));
        }

        if items.len() == 3 {
            // Simple form: (apply fn list)
            self.compile_expr(&items[1], false)?; // fn
            self.compile_expr(&items[2], false)?; // args list
            self.emit(Op::Apply);
        } else {
            // Multi-arg: (apply fn a1 a2 ... list)
            // Desugar to: (apply fn (cons a1 (cons a2 ... list)))
            // Build the cons chain from the end
            let mut arg_list = items[items.len() - 1].clone(); // last arg (must be list)
            for i in (2..items.len() - 1).rev() {
                arg_list = Value::list(vec![Value::symbol("cons"), items[i].clone(), arg_list]);
            }
            self.compile_expr(&items[1], false)?; // fn
            self.compile_expr(&arg_list, false)?; // constructed args list
            self.emit(Op::Apply);
        }
        Ok(())
    }

    /// Compile `(call/cc fn)` or `(call-with-current-continuation fn)`.
    fn compile_call_cc(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() != 2 {
            return Err(LispError::syntax("call/cc requires exactly 1 argument", ""));
        }
        self.compile_expr(&items[1], false)?; // compile the function
        self.emit(Op::CaptureCc);
        if tail {
            self.emit(Op::TailCall(1));
        } else {
            self.emit(Op::Call(1));
        }
        Ok(())
    }

    /// Compile `(guard (var clause ...) body ...)` — R7RS §4.2.7.
    ///
    /// Compiles to: PushHandler → body → PopHandler → Jump(past clauses)
    ///              handler: var bound to exception, evaluate cond-style clauses.
    fn compile_guard(&mut self, items: &[Value], tail: bool) -> Result<(), LispError> {
        if items.len() < 3 {
            return Err(LispError::syntax("guard requires clauses and body", ""));
        }

        // items[1] = (var clause1 clause2 ...)
        let clause_form = items[1]
            .to_list()
            .ok_or_else(|| LispError::syntax("guard clauses must be a list", ""))?;
        if clause_form.is_empty() {
            return Err(LispError::syntax("guard requires at least a variable", ""));
        }
        let var_name = clause_form[0]
            .as_symbol()
            .map_err(|_| LispError::syntax("guard variable must be a symbol", ""))?
            .name()
            .to_string();
        let clauses = &clause_form[1..];

        // Emit PushHandler with placeholder offset
        let handler_idx = self.emit_placeholder(Op::PushHandler(0));

        // Compile body (in the protected region)
        let body = &items[2..];
        self.compile_begin(body, false)?;

        // Normal exit: pop handler and jump past the handler code
        self.emit(Op::PopHandler);
        let jump_past_idx = self.emit_placeholder(Op::Jump(0));

        // Handler starts here — exception value is on top of stack
        let handler_start = self.current_scope().code.current_offset();
        self.patch_jump(handler_idx, handler_start);

        // Bind the exception to var (as a local or global)
        let var_name_ref = var_name.clone();
        if self.scopes.len() > 1 {
            let idx = self.current_scope_mut().add_local(var_name);
            self.emit(Op::StoreLocal(idx));
            self.emit(Op::Const(Value::Void)); // StoreLocal consumed the value
            self.emit(Op::Pop);
        } else {
            self.emit(Op::DefineGlobal(var_name));
            self.emit(Op::Pop); // Pop the Void from define
        }

        // Compile clauses as cond-style: ((test expr ...) ...)
        // Special case: (else expr ...) or (#t expr ...)
        if clauses.is_empty() {
            // No clauses — re-raise
            self.emit(Op::Raise);
        } else {
            self.compile_guard_clauses(clauses, &var_name_ref, tail)?;
        }

        // Patch the jump-past for normal exit
        let after_handler = self.current_scope().code.current_offset();
        self.patch_jump(jump_past_idx, after_handler);

        Ok(())
    }

    fn compile_guard_clauses(
        &mut self,
        clauses: &[Value],
        exn_var: &str,
        tail: bool,
    ) -> Result<(), LispError> {
        // Similar to cond compilation
        let mut jump_to_end_indices = Vec::new();
        // Check if any clause is a catch-all (else or #t)
        let has_catch_all = clauses.iter().any(|c| {
            c.to_list()
                .map(|parts| {
                    !parts.is_empty()
                        && (matches!(&parts[0], Value::Symbol(s) if s.name() == "else")
                            || matches!(&parts[0], Value::Bool(true)))
                })
                .unwrap_or(false)
        });

        for (i, clause) in clauses.iter().enumerate() {
            let is_last = i == clauses.len() - 1;
            let parts = clause
                .to_list()
                .ok_or_else(|| LispError::syntax("guard clause must be a list", ""))?;
            if parts.is_empty() {
                return Err(LispError::syntax("empty guard clause", ""));
            }

            // Check for else clause
            let is_else = matches!(&parts[0], Value::Symbol(s) if s.name() == "else")
                || matches!(&parts[0], Value::Bool(true));

            if is_else {
                // Compile body
                if parts.len() > 1 {
                    self.compile_begin(&parts[1..], tail)?;
                } else {
                    self.emit(Op::Const(Value::Void));
                }
                break;
            }

            // Compile test
            self.compile_expr(&parts[0], false)?;
            let jump_if_false = self.emit_placeholder(Op::JumpIfFalse(0));

            // Compile body (if test is true)
            if parts.len() > 1 {
                self.compile_begin(&parts[1..], tail)?;
            } else {
                self.emit(Op::Const(Value::Bool(true)));
            }

            let j = self.emit_placeholder(Op::Jump(0));
            jump_to_end_indices.push(j);

            let after = self.current_scope().code.current_offset();
            self.patch_jump(jump_if_false, after);

            // If last clause and no else, re-raise the exception
            if is_last && !has_catch_all {
                // Load the exception variable and re-raise
                if let Some(idx) = self.current_scope().resolve_local(exn_var) {
                    self.emit(Op::LoadLocal(idx));
                } else {
                    self.emit(Op::LoadGlobal(exn_var.to_string()));
                }
                self.emit(Op::Raise);
            }
        }

        let end = self.current_scope().code.current_offset();
        for j in jump_to_end_indices {
            self.patch_jump(j, end);
        }

        Ok(())
    }

    /// Compile `(raise obj)` — R7RS §6.11.
    fn compile_raise(&mut self, items: &[Value]) -> Result<(), LispError> {
        if items.len() != 2 {
            return Err(LispError::syntax("raise requires exactly 1 argument", ""));
        }
        self.compile_expr(&items[1], false)?;
        self.emit(Op::Raise);
        Ok(())
    }

    /// Compile `(with-exception-handler handler thunk)` — R7RS §6.11.
    ///
    /// Desugars to: `(let ((%h handler)) (guard (%e (#t (%h %e))) (thunk)))`
    fn compile_with_exception_handler(
        &mut self,
        items: &[Value],
        tail: bool,
    ) -> Result<(), LispError> {
        if items.len() != 3 {
            return Err(LispError::syntax(
                "with-exception-handler requires handler and thunk",
                "",
            ));
        }

        // Desugar to: (let ((%handler <handler-expr>))
        //               (guard (%exn (#t (%handler %exn)))
        //                 (<thunk-expr>)))
        let handler_sym = Value::symbol("%weh-handler");
        let exn_sym = Value::symbol("%weh-exn");

        let desugared = Value::list(vec![
            Value::symbol("let"),
            Value::list(vec![Value::list(vec![
                handler_sym.clone(),
                items[1].clone(),
            ])]),
            Value::list(vec![
                Value::symbol("guard"),
                Value::list(vec![
                    exn_sym.clone(),
                    Value::list(vec![
                        Value::Bool(true),
                        Value::list(vec![handler_sym, exn_sym]),
                    ]),
                ]),
                Value::list(vec![items[2].clone()]),
            ]),
        ]);

        self.compile_expr(&desugared, tail)
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn emit(&mut self, op: Op) {
        self.current_scope_mut().code.emit(op);
    }

    fn emit_placeholder(&mut self, op: Op) -> usize {
        let idx = self.current_scope().code.current_offset();
        self.emit(op);
        idx
    }

    fn current_offset(&self) -> usize {
        self.current_scope().code.current_offset()
    }

    fn patch_jump(&mut self, index: usize, target: usize) {
        self.current_scope_mut().code.patch_jump(index, target);
    }

    fn current_scope(&self) -> &CompileScope {
        self.scopes.last().unwrap()
    }

    fn current_scope_mut(&mut self) -> &mut CompileScope {
        self.scopes.last_mut().unwrap()
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}
