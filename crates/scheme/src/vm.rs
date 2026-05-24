//! mae-scheme virtual machine: bytecode interpreter.
//!
//! Executes compiled bytecode with:
//! - Proper tail calls (TAIL_CALL reuses the current frame)
//! - call/cc support (CAPTURE_CC snapshots the stack)
//! - Yield support (YIELD returns control to Rust)
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use std::collections::HashMap;

use crate::compiler::{CodeObject, Compiler, MacroDef, Op, UpvalueDesc};
use crate::env::Env;
use crate::library::{self, LibraryRegistry};
use crate::lisp_error::{Arity, LispError};
use crate::reader;
use crate::value::{CallFrame, Closure, Continuation, ForeignFn, Value};

/// Result of evaluation — either done or yielding.
pub enum EvalResult {
    Done(Value),
    Yield(YieldRequest, Box<VmState>),
}

/// What the VM wants from the host when it yields.
#[derive(Debug)]
pub enum YieldRequest {
    Sleep(Duration),
}

/// VM state snapshot for resuming after yield.
pub struct VmState {
    pub stack: Vec<Value>,
    pub frames: Vec<Frame>,
    pub globals: Env,
    pub code_pool: Vec<CodeObject>,
}

/// A call frame on the VM stack.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Index into the code pool.
    pub code_id: usize,
    /// Instruction pointer.
    pub ip: usize,
    /// Base pointer (start of locals on the value stack).
    pub bp: usize,
    /// Captured upvalues for this closure invocation.
    pub upvalues: Vec<Rc<RefCell<Value>>>,
    /// Function name for stack traces.
    pub name: Option<String>,
    /// Cells for locals that have been captured as upvalues.
    /// Maps local index → shared cell. Ensures multiple closures
    /// capturing the same local share the same mutable cell.
    pub local_cells: HashMap<usize, Rc<RefCell<Value>>>,
}

/// An exception handler entry on the handler stack.
#[derive(Clone, Debug)]
struct ExceptionHandler {
    /// Code offset to jump to when an exception is raised.
    handler_ip: usize,
    /// Code ID of the frame containing the handler.
    code_id: usize,
    /// Stack depth at time of PushHandler (restore on exception).
    stack_depth: usize,
    /// Frame depth at time of PushHandler.
    frame_depth: usize,
}

/// The virtual machine.
pub struct Vm {
    /// Value stack.
    stack: Vec<Value>,
    /// Call frame stack.
    frames: Vec<Frame>,
    /// Global environment.
    pub globals: Env,
    /// Compiled code objects.
    pub code_pool: Vec<CodeObject>,
    /// Maximum stack depth (prevent infinite recursion crashes).
    max_frames: usize,
    /// Macro definitions persisted across eval calls.
    macros: HashMap<String, MacroDef>,
    /// Library registry for the module system.
    pub libraries: LibraryRegistry,
    /// Exception handler stack.
    handlers: Vec<ExceptionHandler>,
}

impl Vm {
    pub fn new() -> Self {
        Vm {
            stack: Vec::with_capacity(1024),
            frames: Vec::with_capacity(256),
            globals: Env::new(),
            code_pool: Vec::new(),
            max_frames: 10_000,
            macros: HashMap::new(),
            libraries: LibraryRegistry::new(),
            handlers: Vec::new(),
        }
    }

    /// Register a Rust function as a global.
    pub fn register_fn<F>(&mut self, name: &str, doc: &str, arity: Arity, f: F)
    where
        F: Fn(&[Value]) -> Result<Value, LispError> + 'static,
    {
        let foreign = ForeignFn {
            name: name.to_string(),
            func: Box::new(f),
            arity,
            doc: doc.to_string(),
        };
        self.globals
            .define(name.to_string(), Value::Foreign(Rc::new(foreign)));
    }

    /// Define a global variable (updates existing if present).
    pub fn define_global(&mut self, name: &str, value: Value) {
        self.globals.define(name.to_string(), value);
    }

    /// Evaluate a string of Scheme code.
    pub fn eval(&mut self, code: &str) -> Result<Value, LispError> {
        let datums = reader::read_all(code)?;
        if datums.is_empty() {
            return Ok(Value::Void);
        }

        // Pre-process top-level forms: handle import and define-library
        // before compilation, as they affect the global environment.
        let mut to_compile = Vec::new();
        for datum in &datums {
            if self.is_top_level_import(datum) {
                self.process_import(datum)?;
            } else if self.is_define_library(datum) {
                self.process_define_library(datum)?;
            } else {
                to_compile.push(datum.clone());
            }
        }

        if to_compile.is_empty() {
            return Ok(Value::Void);
        }

        let mut compiler = Compiler::new();
        // Seed compiler with macros from previous evals
        compiler.macros = self.macros.clone();

        let code_id = compiler.compile_top_level(&to_compile)?;

        // Persist any new macro definitions back to the VM
        self.macros = compiler.macros;

        // Merge compiled code into VM's pool
        let base = self.code_pool.len();
        // Adjust code_id references in the compiled code
        for mut code_obj in compiler.code_pool {
            // Adjust MakeClosure references
            for op in &mut code_obj.ops {
                if let Op::MakeClosure(ref mut idx, _) = op {
                    *idx += base;
                }
            }
            self.code_pool.push(code_obj);
        }

        self.execute(base + code_id)
    }

    /// Check if a datum is a top-level `(import ...)` form.
    fn is_top_level_import(&self, datum: &Value) -> bool {
        if let Ok(items) = datum.to_vec() {
            if let Some(Value::Symbol(s)) = items.first() {
                return s.name() == "import";
            }
        }
        false
    }

    /// Check if a datum is a `(define-library ...)` form.
    fn is_define_library(&self, datum: &Value) -> bool {
        if let Ok(items) = datum.to_vec() {
            if let Some(Value::Symbol(s)) = items.first() {
                return s.name() == "define-library";
            }
        }
        false
    }

    /// Process a top-level `(import <import-set> ...)` form.
    /// Resolves each import set and binds imported names into globals.
    fn process_import(&mut self, datum: &Value) -> Result<(), LispError> {
        let items = datum
            .to_vec()
            .map_err(|_| LispError::syntax("import must be a list", format!("{datum}")))?;
        let import_sets = library::parse_top_level_import(&items)?;

        for import_set in &import_sets {
            let lib = self.libraries.get(&import_set.library).ok_or_else(|| {
                LispError::syntax(format!("unknown library: {}", import_set.library), "")
            })?;
            let resolved = library::resolve_import(import_set, lib)?;
            for (name, value) in resolved {
                self.globals.define(name, value);
            }
        }

        Ok(())
    }

    /// Process a `(define-library ...)` form.
    /// Evaluates the library body in a fresh scope and registers exports.
    fn process_define_library(&mut self, datum: &Value) -> Result<(), LispError> {
        let items = datum
            .to_vec()
            .map_err(|_| LispError::syntax("define-library must be a list", format!("{datum}")))?;
        let lib_def = library::parse_define_library(&items)?;

        // Check for circular dependency
        if self.libraries.contains(&lib_def.name) {
            return Err(LispError::syntax(
                format!("library already defined: {}", lib_def.name),
                "",
            ));
        }

        // Process imports for the library
        for import_set in &lib_def.imports {
            let lib = self.libraries.get(&import_set.library).ok_or_else(|| {
                LispError::syntax(
                    format!(
                        "library {} requires unknown library: {}",
                        lib_def.name, import_set.library
                    ),
                    "",
                )
            })?;
            let resolved = library::resolve_import(import_set, lib)?;
            for (name, value) in resolved {
                self.globals.define(name, value);
            }
        }

        // Save globals before library body evaluation to isolate internal defs.
        let saved_globals = self.globals.clone();

        // Evaluate the library body
        if !lib_def.body.is_empty() {
            let mut compiler = Compiler::new();
            compiler.macros = self.macros.clone();
            let code_id = compiler.compile_top_level(&lib_def.body)?;
            self.macros = compiler.macros;

            let base = self.code_pool.len();
            for mut code_obj in compiler.code_pool {
                for op in &mut code_obj.ops {
                    if let Op::MakeClosure(ref mut idx, _) = op {
                        *idx += base;
                    }
                }
                self.code_pool.push(code_obj);
            }

            self.execute(base + code_id)?;
        }

        // Collect exports from the global environment (includes library-defined bindings)
        let mut exports = HashMap::new();
        for (export_name, internal_name) in &lib_def.exports {
            if let Some(value) = self.globals.get(internal_name) {
                exports.insert(export_name.clone(), value.clone());
            } else {
                // Restore globals before returning error
                self.globals = saved_globals;
                return Err(LispError::syntax(
                    format!(
                        "library {}: exported name '{}' not defined",
                        lib_def.name, internal_name
                    ),
                    "",
                ));
            }
        }

        // Restore globals — library body definitions don't leak into the global scope.
        // Only exported bindings are available via (import ...).
        self.globals = saved_globals;

        self.libraries.register(library::Library {
            name: lib_def.name,
            exports,
        });

        Ok(())
    }

    /// Execute a code object by index.
    fn execute(&mut self, code_id: usize) -> Result<Value, LispError> {
        // Push initial frame
        self.frames.push(Frame {
            code_id,
            ip: 0,
            bp: self.stack.len(),
            upvalues: Vec::new(),
            name: self.code_pool[code_id].name.clone(),
            local_cells: HashMap::new(),
        });

        self.run()
    }

    /// The main interpreter loop.
    fn run(&mut self) -> Result<Value, LispError> {
        loop {
            if self.frames.is_empty() {
                return Ok(self.stack.pop().unwrap_or(Value::Void));
            }

            let frame = self.frames.last().unwrap();
            let code_id = frame.code_id;
            let ip = frame.ip;

            if ip >= self.code_pool[code_id].ops.len() {
                // End of code — implicit return
                let result = self.stack.pop().unwrap_or(Value::Void);
                let frame = self.frames.pop().unwrap();
                self.stack.truncate(frame.bp);
                self.stack.push(result);
                continue;
            }

            let op = self.code_pool[code_id].ops[ip].clone();
            self.frames.last_mut().unwrap().ip += 1;

            match op {
                Op::Const(val) => {
                    self.stack.push(val);
                }

                Op::LoadGlobal(name) => {
                    let val = self
                        .globals
                        .get(&name)
                        .cloned()
                        .ok_or_else(|| LispError::undefined(&name))?;
                    self.stack.push(val);
                }

                Op::StoreGlobal(name) => {
                    let val = self.stack.pop().unwrap_or(Value::Void);
                    self.globals.define(name, val);
                }

                Op::DefineGlobal(name) => {
                    let val = self.stack.pop().unwrap_or(Value::Void);
                    self.globals.define(name, val);
                }

                Op::LoadLocal(idx) => {
                    let bp = self.frames.last().unwrap().bp;
                    let abs_idx = bp + idx;
                    let val = if abs_idx < self.stack.len() {
                        self.stack[abs_idx].clone()
                    } else {
                        Value::Undefined
                    };
                    self.stack.push(val);
                }

                Op::StoreLocal(idx) => {
                    let val = self.stack.pop().unwrap_or(Value::Void);
                    let bp = self.frames.last().unwrap().bp;
                    let abs_idx = bp + idx;
                    // Extend stack if needed (internal defines create new locals)
                    while abs_idx >= self.stack.len() {
                        self.stack.push(Value::Undefined);
                    }
                    self.stack[abs_idx] = val;
                }

                Op::LoadUpvalue(idx) => {
                    let val = self
                        .frames
                        .last()
                        .unwrap()
                        .upvalues
                        .get(idx)
                        .map(|cell| cell.borrow().clone())
                        .unwrap_or(Value::Undefined);
                    self.stack.push(val);
                }

                Op::StoreUpvalue(idx) => {
                    let val = self.stack.pop().unwrap_or(Value::Void);
                    if let Some(frame) = self.frames.last_mut() {
                        if idx < frame.upvalues.len() {
                            *frame.upvalues[idx].borrow_mut() = val;
                        }
                    }
                }

                Op::Call(argc) => {
                    if let Err(e) = self.do_call(argc, false) {
                        self.handle_exception(e)?;
                    }
                }

                Op::TailCall(argc) => {
                    if let Err(e) = self.do_call(argc, true) {
                        self.handle_exception(e)?;
                    }
                }

                Op::Return => {
                    let result = self.stack.pop().unwrap_or(Value::Void);
                    let frame = self.frames.pop().unwrap();
                    self.stack.truncate(frame.bp);
                    self.stack.push(result);
                }

                Op::Jump(offset) => {
                    let frame = self.frames.last_mut().unwrap();
                    frame.ip = (frame.ip as i32 + offset) as usize;
                }

                Op::JumpIfFalse(offset) => {
                    let val = self.stack.pop().unwrap_or(Value::Bool(false));
                    if !val.is_true() {
                        let frame = self.frames.last_mut().unwrap();
                        frame.ip = (frame.ip as i32 + offset) as usize;
                    }
                }

                Op::Pop => {
                    self.stack.pop();
                }

                Op::Dup => {
                    if let Some(val) = self.stack.last() {
                        self.stack.push(val.clone());
                    }
                }

                Op::MakeClosure(code_id, upvalue_descs) => {
                    let code = &self.code_pool[code_id];
                    let arity = if code.variadic {
                        Arity::Variadic(code.arity)
                    } else {
                        Arity::Fixed(code.arity)
                    };

                    // Capture upvalues as shared mutable cells.
                    // When capturing a local, check if it's already been captured
                    // by another closure in the same scope (local_cells map).
                    // If so, share the same cell so set! is visible to all.
                    let mut upvalues: Vec<Rc<RefCell<Value>>> =
                        Vec::with_capacity(upvalue_descs.len());
                    for desc in &upvalue_descs {
                        let cell = match desc {
                            UpvalueDesc::Local(idx) => {
                                let frame = self.frames.last_mut().unwrap();
                                if let Some(existing) = frame.local_cells.get(idx) {
                                    // Reuse existing cell (shared mutation between
                                    // closures in the same scope)
                                    existing.clone()
                                } else {
                                    let bp = frame.bp;
                                    let abs_idx = bp + idx;
                                    let val = if abs_idx < self.stack.len() {
                                        self.stack[abs_idx].clone()
                                    } else {
                                        Value::Undefined
                                    };
                                    let new_cell = Rc::new(RefCell::new(val));
                                    frame.local_cells.insert(*idx, new_cell.clone());
                                    new_cell
                                }
                            }
                            UpvalueDesc::Upvalue(idx) => {
                                // Share the same cell from the enclosing closure
                                self.frames
                                    .last()
                                    .unwrap()
                                    .upvalues
                                    .get(*idx)
                                    .cloned()
                                    .unwrap_or_else(|| Rc::new(RefCell::new(Value::Undefined)))
                            }
                        };
                        upvalues.push(cell);
                    }

                    let closure = Closure {
                        code_id,
                        upvalues,
                        arity,
                        name: code.name.clone(),
                        doc: None,
                    };
                    self.stack.push(Value::Closure(Rc::new(closure)));
                }

                Op::CaptureCc => {
                    // CaptureCc is emitted before Call(1) by compile_call_cc.
                    // We capture a continuation that, when invoked with a value,
                    // restores state so the value becomes the result of the
                    // call/cc expression (i.e., skipping the Call(1) that follows).
                    //
                    // The current frame IP has already advanced past CaptureCc.
                    // The next instruction will be Call(1) or TailCall(1).
                    // We need the continuation to skip that Call, so we advance
                    // the captured IP by 1 more instruction.
                    let mut captured_frames: Vec<CallFrame> = self
                        .frames
                        .iter()
                        .map(|f| CallFrame {
                            code_id: f.code_id,
                            ip: f.ip,
                            bp: f.bp,
                            function_name: f.name.clone(),
                        })
                        .collect();

                    // Advance the top frame's IP past the upcoming Call(1)/TailCall(1)
                    if let Some(top) = captured_frames.last_mut() {
                        top.ip += 1; // skip the Call(1) that follows CaptureCc
                    }

                    // Capture stack WITHOUT the function that's on top
                    // (the fn is for Call(1) to consume, not part of the continuation)
                    let fn_on_stack = self.stack.len(); // fn is at top
                    let captured_stack = self.stack[..fn_on_stack - 1].to_vec();

                    let cont = Continuation {
                        stack: captured_stack,
                        frames: captured_frames,
                        invoked: false,
                    };
                    // Push continuation as argument to the function (for Call(1))
                    self.stack.push(Value::Continuation(Rc::new(cont)));
                }

                Op::Yield => {
                    // For now, only Sleep yield
                    let duration = self.stack.pop().unwrap_or(Value::Int(0));
                    let ms = duration.as_int().unwrap_or(0) as u64;
                    let state = VmState {
                        stack: std::mem::take(&mut self.stack),
                        frames: std::mem::take(&mut self.frames),
                        globals: std::mem::take(&mut self.globals),
                        code_pool: std::mem::take(&mut self.code_pool),
                    };
                    // This would return EvalResult::Yield in the resumable API
                    // For now, just sleep and continue
                    std::thread::sleep(Duration::from_millis(ms));
                    self.stack = state.stack;
                    self.frames = state.frames;
                    self.globals = state.globals;
                    self.code_pool = state.code_pool;
                    self.stack.push(Value::Bool(true));
                }

                Op::Nop => {}

                Op::Apply => {
                    // Stack: [fn, args-list]
                    let args_list = self.stack.pop().unwrap_or(Value::Void);
                    let func = self.stack.pop().unwrap_or(Value::Void);

                    // Convert the args list to a vector
                    let mut args = Vec::new();
                    let mut current = args_list;
                    loop {
                        match current {
                            Value::Pair(pair) => {
                                args.push(pair.0.clone());
                                current = pair.1.clone();
                            }
                            Value::Null => break,
                            Value::Void => break,
                            _ => {
                                return Err(LispError::type_error(
                                    "proper list",
                                    current.type_name(),
                                ));
                            }
                        }
                    }

                    // Push function and args onto stack, then call
                    let argc = args.len();
                    self.stack.push(func);
                    for a in args {
                        self.stack.push(a);
                    }
                    self.do_call(argc, false)?;
                }

                Op::PushHandler(offset) => {
                    let frame = self.frames.last().unwrap();
                    let handler_ip = (frame.ip as i32 + offset) as usize;
                    self.handlers.push(ExceptionHandler {
                        handler_ip,
                        code_id: frame.code_id,
                        stack_depth: self.stack.len(),
                        frame_depth: self.frames.len(),
                    });
                }

                Op::PopHandler => {
                    self.handlers.pop();
                }

                Op::Raise => {
                    let exception = self.stack.pop().unwrap_or(Value::Void);
                    if let Some(handler) = self.handlers.pop() {
                        // Unwind stack and frames to handler's state
                        self.stack.truncate(handler.stack_depth);
                        self.frames.truncate(handler.frame_depth);
                        // Push exception value for the handler code
                        self.stack.push(exception);
                        // Jump to handler
                        if let Some(frame) = self.frames.last_mut() {
                            frame.ip = handler.handler_ip;
                            frame.code_id = handler.code_id;
                        }
                    } else {
                        // No handler — convert to Rust error
                        let msg = match &exception {
                            Value::String(s) => s.to_string(),
                            other => format!("{other}"),
                        };
                        return Err(LispError::internal(format!("unhandled exception: {msg}")));
                    }
                }

                Op::Values | Op::CallWithValues => {
                    return Err(LispError::internal(format!("unimplemented opcode: {op:?}")));
                }
            }
        }
    }

    /// Handle a Rust-level error by dispatching to exception handlers or propagating.
    fn handle_exception(&mut self, err: LispError) -> Result<(), LispError> {
        if let Some(handler) = self.handlers.pop() {
            // Unwind to handler state
            self.stack.truncate(handler.stack_depth);
            self.frames.truncate(handler.frame_depth);
            // Push the error message as the exception value
            let exception = Value::string(err.message());
            self.stack.push(exception);
            // Jump to handler code
            if let Some(frame) = self.frames.last_mut() {
                frame.ip = handler.handler_ip;
                frame.code_id = handler.code_id;
            }
            Ok(())
        } else {
            Err(err)
        }
    }

    /// Handle function calls (both regular and tail calls).
    fn do_call(&mut self, argc: usize, tail: bool) -> Result<(), LispError> {
        if self.stack.len() < argc + 1 {
            return Err(LispError::internal("stack underflow in call"));
        }

        // Get the function and arguments from the stack
        let fn_pos = self.stack.len() - argc - 1;
        let func = self.stack[fn_pos].clone();

        match func {
            Value::Closure(closure) => {
                // Check arity
                match &closure.arity {
                    Arity::Fixed(n) if argc != *n => {
                        return Err(LispError::arity(
                            closure.name.as_deref().unwrap_or("<lambda>"),
                            Arity::Fixed(*n),
                            argc,
                        ));
                    }
                    Arity::Variadic(min) if argc < *min => {
                        return Err(LispError::arity(
                            closure.name.as_deref().unwrap_or("<lambda>"),
                            Arity::Variadic(*min),
                            argc,
                        ));
                    }
                    _ => {}
                }

                // Collect arguments
                let args: Vec<Value> = self.stack[fn_pos + 1..].to_vec();

                if tail {
                    // Tail call: reuse current frame
                    let frame = self.frames.last_mut().unwrap();
                    // Truncate stack to frame's base pointer
                    self.stack.truncate(frame.bp);

                    // Handle variadic: pack extra args into a list
                    if let Arity::Variadic(min) = &closure.arity {
                        let min = *min;
                        for arg in &args[..min] {
                            self.stack.push(arg.clone());
                        }
                        // Pack rest into a list
                        let rest = Value::list(args[min..].iter().cloned());
                        self.stack.push(rest);
                    } else {
                        for arg in &args {
                            self.stack.push(arg.clone());
                        }
                    }

                    frame.code_id = closure.code_id;
                    frame.ip = 0;
                    frame.bp = self.stack.len()
                        - if let Arity::Variadic(min) = &closure.arity {
                            min + 1
                        } else {
                            argc
                        };
                    frame.upvalues = closure.upvalues.clone();
                    frame.name = closure.name.clone();
                    frame.local_cells.clear(); // Reset for new invocation

                    // Fix bp: it should be the start of args on the stack
                    frame.bp = self.stack.len()
                        - if let Arity::Variadic(min) = &closure.arity {
                            min + 1
                        } else {
                            argc
                        };
                } else {
                    if self.frames.len() >= self.max_frames {
                        return Err(LispError::internal(format!(
                            "stack overflow: {} frames",
                            self.max_frames
                        )));
                    }

                    // Remove function and args from stack
                    self.stack.truncate(fn_pos);

                    let bp = self.stack.len();

                    // Push args (handle variadic)
                    if let Arity::Variadic(min) = &closure.arity {
                        let min = *min;
                        for arg in &args[..min] {
                            self.stack.push(arg.clone());
                        }
                        let rest = Value::list(args[min..].iter().cloned());
                        self.stack.push(rest);
                    } else {
                        for arg in &args {
                            self.stack.push(arg.clone());
                        }
                    }

                    self.frames.push(Frame {
                        code_id: closure.code_id,
                        ip: 0,
                        bp,
                        upvalues: closure.upvalues.clone(),
                        name: closure.name.clone(),
                        local_cells: HashMap::new(),
                    });
                }
            }

            Value::Foreign(ff) => {
                let args: Vec<Value> = self.stack[fn_pos + 1..].to_vec();
                self.stack.truncate(fn_pos);
                let result = (ff.func)(&args)?;
                self.stack.push(result);
            }

            Value::Continuation(cont) => {
                // Invoking a continuation: restore captured state
                if argc != 1 {
                    return Err(LispError::arity("<continuation>", Arity::Fixed(1), argc));
                }
                let val = self.stack.pop().unwrap_or(Value::Void);
                self.stack.truncate(fn_pos);

                // Restore continuation state
                self.stack = cont.stack.clone();
                self.frames = cont
                    .frames
                    .iter()
                    .map(|cf| Frame {
                        code_id: cf.code_id,
                        ip: cf.ip,
                        bp: cf.bp,
                        upvalues: Vec::new(),
                        local_cells: HashMap::new(),
                        name: cf.function_name.clone(),
                    })
                    .collect();

                // Push the value as the result
                self.stack.push(val);
            }

            _ => {
                return Err(LispError::type_error("procedure", func.type_name()));
            }
        }

        Ok(())
    }

    /// Get current stack trace for debugging.
    pub fn stack_trace(&self) -> Vec<(Option<String>, Option<SourceLocation>)> {
        self.frames
            .iter()
            .rev()
            .map(|f| {
                let loc = self.code_pool.get(f.code_id).and_then(|code| {
                    if f.ip > 0 {
                        code.source_map.get(f.ip - 1).cloned().flatten()
                    } else {
                        None
                    }
                });
                (f.name.clone(), loc)
            })
            .collect()
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

use crate::lisp_error::SourceLocation;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(code: &str) -> Value {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(code).unwrap()
    }

    fn eval_err(code: &str) -> String {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(code).unwrap_err().message()
    }

    /// Register minimal builtins for testing.
    fn register_builtins(vm: &mut Vm) {
        vm.register_fn("+", "Add numbers", Arity::Variadic(0), |args| {
            let mut sum = 0i64;
            let mut is_float = false;
            let mut fsum = 0.0f64;
            for arg in args {
                match arg {
                    Value::Int(n) => {
                        if is_float {
                            fsum += *n as f64;
                        } else {
                            sum += n;
                        }
                    }
                    Value::Float(n) => {
                        if !is_float {
                            fsum = sum as f64;
                            is_float = true;
                        }
                        fsum += n;
                    }
                    _ => return Err(LispError::type_error("number", arg.type_name())),
                }
            }
            if is_float {
                Ok(Value::Float(fsum))
            } else {
                Ok(Value::Int(sum))
            }
        });

        vm.register_fn("-", "Subtract numbers", Arity::Variadic(1), |args| {
            if args.len() == 1 {
                return match &args[0] {
                    Value::Int(n) => Ok(Value::Int(-n)),
                    Value::Float(n) => Ok(Value::Float(-n)),
                    _ => Err(LispError::type_error("number", args[0].type_name())),
                };
            }
            let first = args[0].as_float()?;
            let mut result = first;
            for arg in &args[1..] {
                result -= arg.as_float()?;
            }
            if args.iter().all(|a| matches!(a, Value::Int(_))) {
                Ok(Value::Int(result as i64))
            } else {
                Ok(Value::Float(result))
            }
        });

        vm.register_fn("*", "Multiply numbers", Arity::Variadic(0), |args| {
            let mut product = 1i64;
            let mut is_float = false;
            let mut fproduct = 1.0f64;
            for arg in args {
                match arg {
                    Value::Int(n) => {
                        if is_float {
                            fproduct *= *n as f64;
                        } else {
                            product *= n;
                        }
                    }
                    Value::Float(n) => {
                        if !is_float {
                            fproduct = product as f64;
                            is_float = true;
                        }
                        fproduct *= n;
                    }
                    _ => return Err(LispError::type_error("number", arg.type_name())),
                }
            }
            if is_float {
                Ok(Value::Float(fproduct))
            } else {
                Ok(Value::Int(product))
            }
        });

        vm.register_fn("=", "Numeric equality", Arity::Variadic(2), |args| {
            let first = args[0].as_float()?;
            for arg in &args[1..] {
                if arg.as_float()? != first {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        });

        vm.register_fn("<", "Less than", Arity::Variadic(2), |args| {
            for w in args.windows(2) {
                if w[0].as_float()? >= w[1].as_float()? {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        });

        vm.register_fn(">", "Greater than", Arity::Variadic(2), |args| {
            for w in args.windows(2) {
                if w[0].as_float()? <= w[1].as_float()? {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        });

        vm.register_fn("<=", "Less or equal", Arity::Variadic(2), |args| {
            for w in args.windows(2) {
                if w[0].as_float()? > w[1].as_float()? {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        });

        vm.register_fn(">=", "Greater or equal", Arity::Variadic(2), |args| {
            for w in args.windows(2) {
                if w[0].as_float()? < w[1].as_float()? {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        });

        vm.register_fn("not", "Boolean not", Arity::Fixed(1), |args| {
            Ok(Value::Bool(!args[0].is_true()))
        });

        vm.register_fn("cons", "Construct pair", Arity::Fixed(2), |args| {
            Ok(Value::cons(args[0].clone(), args[1].clone()))
        });

        vm.register_fn("car", "First of pair", Arity::Fixed(1), |args| {
            args[0].car()
        });

        vm.register_fn("cdr", "Rest of pair", Arity::Fixed(1), |args| args[0].cdr());

        vm.register_fn("null?", "Is null?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(args[0].is_null()))
        });

        vm.register_fn("pair?", "Is pair?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(args[0].is_pair()))
        });

        vm.register_fn("list", "Construct list", Arity::Variadic(0), |args| {
            Ok(Value::list(args.iter().cloned()))
        });

        vm.register_fn("display", "Display value", Arity::Fixed(1), |args| {
            print!("{}", crate::value::display_value(&args[0]));
            Ok(Value::Void)
        });

        vm.register_fn("newline", "Print newline", Arity::Fixed(0), |_| {
            println!();
            Ok(Value::Void)
        });

        vm.register_fn("eq?", "Identity equality", Arity::Fixed(2), |args| {
            Ok(Value::Bool(args[0] == args[1]))
        });

        vm.register_fn("number?", "Is number?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(args[0].is_number()))
        });

        vm.register_fn("string?", "Is string?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(args[0].is_string()))
        });

        vm.register_fn("symbol?", "Is symbol?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(args[0].is_symbol()))
        });

        vm.register_fn("procedure?", "Is procedure?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(args[0].is_procedure()))
        });

        vm.register_fn("boolean?", "Is boolean?", Arity::Fixed(1), |args| {
            Ok(Value::Bool(matches!(args[0], Value::Bool(_))))
        });

        vm.register_fn(
            "apply",
            "Apply function to args",
            Arity::Variadic(2),
            |_args| {
                // (apply f arg1 ... args-list)
                // Not fully implementable as a foreign fn since it needs the VM.
                // This is a stub — real apply is handled in the VM loop.
                Err(LispError::internal(
                    "apply must be called from Scheme, not as a foreign function",
                ))
            },
        );

        vm.register_fn("error", "Raise an error", Arity::Variadic(1), |args| {
            let msg = if args[0].is_string() {
                args[0].as_str().unwrap().to_string()
            } else {
                format!("{}", args[0])
            };
            let irritants: Vec<String> = args[1..].iter().map(|a| format!("{a}")).collect();
            Err(LispError::user(msg, irritants))
        });
    }

    // --- Basic expressions ---

    #[test]
    fn test_constants() {
        assert_eq!(eval("42"), Value::Int(42));
        assert_eq!(eval("#t"), Value::Bool(true));
        assert_eq!(eval("\"hello\""), Value::string("hello"));
    }

    #[test]
    fn test_arithmetic() {
        assert_eq!(eval("(+ 1 2 3)"), Value::Int(6));
        assert_eq!(eval("(* 2 3)"), Value::Int(6));
        assert_eq!(eval("(- 10 3)"), Value::Int(7));
        assert_eq!(eval("(- 5)"), Value::Int(-5));
    }

    #[test]
    fn test_comparison() {
        assert_eq!(eval("(< 1 2)"), Value::Bool(true));
        assert_eq!(eval("(> 1 2)"), Value::Bool(false));
        assert_eq!(eval("(= 1 1)"), Value::Bool(true));
        assert_eq!(eval("(<= 1 1)"), Value::Bool(true));
    }

    #[test]
    fn test_if() {
        assert_eq!(eval("(if #t 1 2)"), Value::Int(1));
        assert_eq!(eval("(if #f 1 2)"), Value::Int(2));
        assert_eq!(eval("(if #t 42)"), Value::Int(42));
    }

    #[test]
    fn test_quote() {
        assert_eq!(eval("'foo").as_symbol().unwrap().name(), "foo");
        let list = eval("'(1 2 3)");
        assert_eq!(list.to_vec().unwrap().len(), 3);
    }

    // --- Variables ---

    #[test]
    fn test_define_and_ref() {
        assert_eq!(eval("(define x 42) x"), Value::Int(42));
    }

    #[test]
    fn test_set() {
        assert_eq!(eval("(define x 1) (set! x 2) x"), Value::Int(2));
    }

    #[test]
    fn test_undefined_variable() {
        let err = eval_err("nonexistent");
        assert!(err.contains("undefined"));
    }

    // --- Functions ---

    #[test]
    fn test_lambda_call() {
        assert_eq!(eval("((lambda (x) (+ x 1)) 5)"), Value::Int(6));
    }

    #[test]
    fn test_define_function() {
        assert_eq!(eval("(define (add1 x) (+ x 1)) (add1 10)"), Value::Int(11));
    }

    #[test]
    fn test_higher_order() {
        assert_eq!(
            eval("(define (apply-twice f x) (f (f x))) (apply-twice (lambda (x) (+ x 1)) 0)"),
            Value::Int(2)
        );
    }

    #[test]
    fn test_closure() {
        assert_eq!(
            eval("(define (make-adder n) (lambda (x) (+ x n))) ((make-adder 10) 5)"),
            Value::Int(15)
        );
    }

    #[test]
    fn test_variadic() {
        let result = eval("(define (f x . rest) rest) (f 1 2 3)");
        let vec = result.to_vec().unwrap();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0], Value::Int(2));
        assert_eq!(vec[1], Value::Int(3));
    }

    // --- Tail calls ---

    #[test]
    fn test_tco_simple() {
        // This should complete without stack overflow
        let result = eval(
            "(define (count n)
               (if (= n 0) 'done (count (- n 1))))
             (count 100000)",
        );
        assert_eq!(result.as_symbol().unwrap().name(), "done");
    }

    #[test]
    fn test_tco_mutual() {
        let result = eval(
            "(define (even? n)
               (if (= n 0) #t (odd? (- n 1))))
             (define (odd? n)
               (if (= n 0) #f (even? (- n 1))))
             (even? 100000)",
        );
        assert_eq!(result, Value::Bool(true));
    }

    // --- Let forms ---

    #[test]
    fn test_let() {
        assert_eq!(eval("(let ((x 1) (y 2)) (+ x y))"), Value::Int(3));
    }

    #[test]
    fn test_let_star() {
        assert_eq!(eval("(let* ((x 1) (y (+ x 1))) y)"), Value::Int(2));
    }

    #[test]
    fn test_letrec() {
        assert_eq!(
            eval("(letrec ((f (lambda (n) (if (= n 0) 1 (* n (f (- n 1))))))) (f 5))"),
            Value::Int(120)
        );
    }

    #[test]
    fn test_named_let() {
        assert_eq!(
            eval(
                "(let loop ((n 10) (acc 0))
                    (if (= n 0) acc (loop (- n 1) (+ acc n))))"
            ),
            Value::Int(55)
        );
    }

    // --- Control flow ---

    #[test]
    fn test_and() {
        assert_eq!(eval("(and)"), Value::Bool(true));
        assert_eq!(eval("(and 1 2 3)"), Value::Int(3));
        assert_eq!(eval("(and 1 #f 3)"), Value::Bool(false));
    }

    #[test]
    fn test_or() {
        assert_eq!(eval("(or)"), Value::Bool(false));
        assert_eq!(eval("(or #f #f 3)"), Value::Int(3));
        assert_eq!(eval("(or 1 2)"), Value::Int(1));
    }

    #[test]
    fn test_cond() {
        assert_eq!(eval("(cond (#f 1) (#t 2) (else 3))"), Value::Int(2));
        assert_eq!(eval("(cond (#f 1) (else 42))"), Value::Int(42));
    }

    #[test]
    fn test_when() {
        assert_eq!(eval("(when #t 42)"), Value::Int(42));
        assert_eq!(eval("(when #f 42)"), Value::Void);
    }

    #[test]
    fn test_unless() {
        assert_eq!(eval("(unless #f 42)"), Value::Int(42));
        assert_eq!(eval("(unless #t 42)"), Value::Void);
    }

    // --- Begin ---

    #[test]
    fn test_begin() {
        assert_eq!(eval("(begin 1 2 3)"), Value::Int(3));
    }

    // --- List operations ---

    #[test]
    fn test_cons_car_cdr() {
        assert_eq!(eval("(car (cons 1 2))"), Value::Int(1));
        assert_eq!(eval("(cdr (cons 1 2))"), Value::Int(2));
    }

    #[test]
    fn test_list_builtin() {
        let result = eval("(list 1 2 3)");
        assert_eq!(result.to_vec().unwrap().len(), 3);
    }

    #[test]
    fn test_null_check() {
        assert_eq!(eval("(null? '())"), Value::Bool(true));
        assert_eq!(eval("(null? 1)"), Value::Bool(false));
    }

    // --- Predicates ---

    #[test]
    fn test_predicates() {
        assert_eq!(eval("(number? 42)"), Value::Bool(true));
        assert_eq!(eval("(string? \"hi\")"), Value::Bool(true));
        assert_eq!(eval("(symbol? 'foo)"), Value::Bool(true));
        assert_eq!(eval("(boolean? #t)"), Value::Bool(true));
        assert_eq!(eval("(procedure? +)"), Value::Bool(true));
    }

    // --- Error handling ---

    #[test]
    fn test_arity_error() {
        // Fixed arity function called with wrong number of args
        let err = eval_err("((lambda (x) x) 1 2)");
        assert!(err.contains("expected 1") || err.contains("arity"));
    }

    #[test]
    fn test_type_error() {
        let err = eval_err("(+ 1 \"hello\")");
        assert!(err.contains("number") || err.contains("type"));
    }

    #[test]
    fn test_user_error() {
        let err = eval_err("(error \"bad\" 42)");
        assert!(err.contains("bad"));
    }

    // --- Void in tail position (Steel regression) ---

    #[test]
    fn test_void_in_tail() {
        // This was a crash in Steel
        let result = eval("(define (f) (if #t (begin 42))) (f)");
        assert_eq!(result, Value::Int(42));
    }

    // --- Multiple expressions ---

    #[test]
    fn test_multiple_top_level() {
        assert_eq!(eval("1 2 3"), Value::Int(3));
    }

    // --- Fibonacci benchmark ---

    #[test]
    fn test_fibonacci() {
        let result = eval(
            "(define (fib n)
               (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))
             (fib 20)",
        );
        assert_eq!(result, Value::Int(6765));
    }

    // --- Complex programs ---

    #[test]
    fn test_map() {
        let result = eval(
            "(define (map f lst)
               (if (null? lst)
                   '()
                   (cons (f (car lst)) (map f (cdr lst)))))
             (map (lambda (x) (* x x)) '(1 2 3 4 5))",
        );
        let vec = result.to_vec().unwrap();
        assert_eq!(vec.len(), 5);
        assert_eq!(vec[0], Value::Int(1));
        assert_eq!(vec[4], Value::Int(25));
    }

    #[test]
    fn test_filter() {
        let result = eval(
            "(define (filter pred lst)
               (cond ((null? lst) '())
                     ((pred (car lst)) (cons (car lst) (filter pred (cdr lst))))
                     (else (filter pred (cdr lst)))))
             (filter (lambda (x) (> x 2)) '(1 2 3 4 5))",
        );
        let vec = result.to_vec().unwrap();
        assert_eq!(vec.len(), 3);
        assert_eq!(vec[0], Value::Int(3));
    }

    #[test]
    fn test_fold() {
        let result = eval(
            "(define (fold f init lst)
               (if (null? lst) init
                   (fold f (f init (car lst)) (cdr lst))))
             (fold + 0 '(1 2 3 4 5))",
        );
        assert_eq!(result, Value::Int(15));
    }

    // --- Module system ---

    #[test]
    fn test_define_library_and_import() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (test math)
               (export square cube)
               (begin
                 (define (square x) (* x x))
                 (define (cube x) (* x x x))))",
        )
        .unwrap();
        vm.eval("(import (test math))").unwrap();
        assert_eq!(vm.eval("(square 5)").unwrap(), Value::Int(25));
        assert_eq!(vm.eval("(cube 3)").unwrap(), Value::Int(27));
    }

    #[test]
    fn test_import_only() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (test stuff)
               (export a b c)
               (begin
                 (define a 1) (define b 2) (define c 3)))",
        )
        .unwrap();
        vm.eval("(import (only (test stuff) a c))").unwrap();
        assert_eq!(vm.eval("a").unwrap(), Value::Int(1));
        assert_eq!(vm.eval("c").unwrap(), Value::Int(3));
        // b should not be imported
        assert!(vm.eval("b").is_err());
    }

    #[test]
    fn test_import_rename() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (test stuff)
               (export a)
               (begin (define a 42)))",
        )
        .unwrap();
        vm.eval("(import (rename (test stuff) (a my-a)))").unwrap();
        assert_eq!(vm.eval("my-a").unwrap(), Value::Int(42));
    }

    #[test]
    fn test_import_prefix() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (test stuff)
               (export x)
               (begin (define x 99)))",
        )
        .unwrap();
        vm.eval("(import (prefix (test stuff) t:))").unwrap();
        assert_eq!(vm.eval("t:x").unwrap(), Value::Int(99));
    }

    #[test]
    fn test_library_export_rename() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (test rename)
               (export (rename internal-fn public-fn))
               (begin (define (internal-fn x) (+ x 1))))",
        )
        .unwrap();
        vm.eval("(import (test rename))").unwrap();
        assert_eq!(vm.eval("(public-fn 10)").unwrap(), Value::Int(11));
    }

    #[test]
    fn test_library_depends_on_library() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (base-lib)
               (export add1)
               (begin (define (add1 x) (+ x 1))))",
        )
        .unwrap();
        vm.eval(
            "(define-library (higher-lib)
               (export add2)
               (import (base-lib))
               (begin (define (add2 x) (add1 (add1 x)))))",
        )
        .unwrap();
        vm.eval("(import (higher-lib))").unwrap();
        assert_eq!(vm.eval("(add2 10)").unwrap(), Value::Int(12));
    }

    #[test]
    fn test_unknown_library_error() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        let err = vm.eval("(import (nonexistent lib))").unwrap_err();
        assert!(err.message().contains("unknown library"));
    }

    #[test]
    fn test_duplicate_library_error() {
        let mut vm = Vm::new();
        register_builtins(&mut vm);
        vm.eval(
            "(define-library (dup)
               (export a)
               (begin (define a 1)))",
        )
        .unwrap();
        let err = vm
            .eval(
                "(define-library (dup)
               (export b)
               (begin (define b 2)))",
            )
            .unwrap_err();
        assert!(err.message().contains("already defined"));
    }
}
