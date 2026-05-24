//! mae-scheme compiler: AST → bytecode.
//!
//! Compiles Scheme expressions into a linear bytecode sequence.
//! The compiler tracks tail position to emit TAIL_CALL for proper
//! tail calls (R7RS §3.5).
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use crate::lisp_error::{LispError, SourceLocation};
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

/// The compiler: transforms Value AST into bytecode CodeObjects.
pub struct Compiler {
    /// Pool of compiled code objects (functions).
    pub code_pool: Vec<CodeObject>,
    /// Stack of compilation scopes (for nested functions).
    scopes: Vec<CompileScope>,
}

impl Compiler {
    pub fn new() -> Self {
        Compiler {
            code_pool: Vec::new(),
            scopes: vec![CompileScope::new()],
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
                        _ => {}
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

        let scope = &self.current_scope();
        let saved_locals = scope.locals.len();
        let saved_depth = scope.scope_depth;
        self.current_scope_mut().scope_depth += 1;

        // Evaluate all init expressions and bind
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

            self.compile_expr(&pair[1], false)?;
            self.current_scope_mut().add_local(name);
        }

        // Compile body
        self.compile_begin(&items[2..], tail)?;

        // Pop locals (no explicit instruction needed — locals live on the stack)
        let to_pop = self.current_scope().locals.len() - saved_locals;
        // We need to save the result, pop the bindings, then push result back
        if to_pop > 0 {
            // The result is on top of stack, with `to_pop` locals below it
            // We'll handle this in the VM by adjusting the stack after let
        }

        self.current_scope_mut().locals.truncate(saved_locals);
        self.current_scope_mut().scope_depth = saved_depth;

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
        if items.len() < 3 {
            return Err(LispError::syntax("let* requires bindings and body", ""));
        }

        let bindings = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("let* bindings must be a list", ""))?;

        let saved_locals = self.current_scope().locals.len();
        let saved_depth = self.current_scope().scope_depth;
        self.current_scope_mut().scope_depth += 1;

        // Sequential binding: each binding sees previous ones
        for binding in &bindings {
            let pair = binding
                .to_vec()
                .map_err(|_| LispError::syntax("let* binding must be (var expr)", ""))?;
            if pair.len() != 2 {
                return Err(LispError::syntax("let* binding must be (var expr)", ""));
            }
            let name = pair[0]
                .as_symbol()
                .map_err(|_| LispError::syntax("let* variable must be a symbol", ""))?
                .name()
                .to_string();
            self.compile_expr(&pair[1], false)?;
            self.current_scope_mut().add_local(name);
        }

        self.compile_begin(&items[2..], tail)?;

        self.current_scope_mut().locals.truncate(saved_locals);
        self.current_scope_mut().scope_depth = saved_depth;

        Ok(())
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

    fn compile_and(&mut self, exprs: &[Value], _tail: bool) -> Result<(), LispError> {
        if exprs.is_empty() {
            self.emit(Op::Const(Value::Bool(true)));
            return Ok(());
        }

        let mut end_jumps = Vec::new();
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            self.compile_expr(expr, false)?;
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

    fn compile_or(&mut self, exprs: &[Value], _tail: bool) -> Result<(), LispError> {
        if exprs.is_empty() {
            self.emit(Op::Const(Value::Bool(false)));
            return Ok(());
        }

        let mut end_jumps = Vec::new();
        for (i, expr) in exprs.iter().enumerate() {
            let is_last = i == exprs.len() - 1;
            self.compile_expr(expr, false)?;
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
        // For now, simple case: (define-values (x) expr) → (define x expr)
        let formals = items[1]
            .to_vec()
            .map_err(|_| LispError::syntax("define-values formals must be a list", ""))?;
        if formals.len() == 1 {
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
            Err(LispError::syntax(
                "define-values with multiple values not yet supported",
                "",
            ))
        }
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
