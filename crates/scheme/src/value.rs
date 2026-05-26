//! mae-scheme value representation.
//!
//! Tagged union for all Scheme values. Uses Rc for heap-allocated data.
//!
//! ## GC Strategy (Stage 1: Rc)
//!
//! **Current approach**: `Rc<T>` for shared heap values (closures, pairs,
//! vectors, continuations). Mutable locals shared across closures use
//! `Rc<RefCell<Value>>` cells.
//!
//! **Known cycle risks** (memory leaks, NOT pauses — Rc has no stop-the-world):
//! - Closure self-capture: `(let ((f #f)) (set! f (lambda () f)))` — closure
//!   captures its own upvalue cell, forming Rc→RefCell→Value→Rc cycle.
//! - Vector→closure: `(let* ((v (vector #f)) (f (lambda () v))) (vector-set! v 0 f))`
//! - Continuation upvalue capture: call/cc captures stack with live upvalue cells.
//!
//! **Why this is acceptable for v1**: Editor extensions are short-lived evals
//! (keystroke handlers, hooks, mode functions). Memory is dominated by the
//! editor's Rust heap, not Scheme values. Emacs ran for 30+ years with
//! mark-sweep GC that pauses the UI; Rc with no pauses is strictly better.
//!
//! **Stage 2 path**: Switch upvalue cells from `Rc<RefCell<Value>>` to a
//! traced GC type (gc-arena or bacon-rajan-cc). The `Trace` trait is defined
//! from day one so this is a backend swap, not an architecture rewrite.
//!
//! **UI responsiveness guarantee**: Rc deallocation is amortized (no pauses).
//! Deep recursion is bounded by VM max_frames limit. There is no tracing GC
//! to cause stop-the-world pauses.
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use crate::lisp_error::{Arity, LispError};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// GC interface (constant across all GC stages)
// ---------------------------------------------------------------------------

/// Trait for GC-traceable values. Stage 1 (Rc): no-op implementations.
/// Stage 2+ (gc-arena or mark-sweep): trace reachable children.
pub trait Trace {
    fn trace(&self, _tracer: &mut dyn Tracer);
}

/// Tracer callback — called by Trace::trace for each reachable child.
pub trait Tracer {
    fn trace_value(&mut self, value: &Value);
}

// ---------------------------------------------------------------------------
// Symbol interning
// ---------------------------------------------------------------------------

/// Interned symbol — pointer equality for `eq?`.
#[derive(Clone, Debug)]
pub struct InternedSymbol {
    id: u32,
    name: Rc<str>,
}

impl InternedSymbol {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> u32 {
        self.id
    }
}

impl PartialEq for InternedSymbol {
    fn eq(&self, other: &Self) -> bool {
        // Compare by name, not ID. Within a single VM, same-name symbols share
        // the same ID (fast path via Rc::ptr_eq on name). Across VMs, name
        // comparison is the correct R7RS §6.5 semantics: symbols with the same
        // spelling are equal.
        self.id == other.id || self.name == other.name
    }
}

impl Eq for InternedSymbol {}

impl std::hash::Hash for InternedSymbol {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Must hash by name to satisfy the Hash contract: equal values must
        // have equal hashes. PartialEq compares by name for cross-VM safety.
        self.name.hash(state);
    }
}

/// Global symbol table for interning.
pub struct SymbolTable {
    by_name: HashMap<Rc<str>, u32>,
    by_id: Vec<Rc<str>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        SymbolTable {
            by_name: HashMap::new(),
            by_id: Vec::new(),
        }
    }

    /// Intern a symbol name, returning an InternedSymbol.
    /// Same name always returns same id (pointer equality).
    pub fn intern(&mut self, name: &str) -> InternedSymbol {
        if let Some(&id) = self.by_name.get(name) {
            InternedSymbol {
                id,
                name: self.by_id[id as usize].clone(),
            }
        } else {
            let id = self.by_id.len() as u32;
            let rc_name: Rc<str> = Rc::from(name);
            self.by_name.insert(rc_name.clone(), id);
            self.by_id.push(rc_name.clone());
            InternedSymbol { id, name: rc_name }
        }
    }

    /// Look up a symbol by id.
    pub fn lookup(&self, id: u32) -> Option<&str> {
        self.by_id.get(id as usize).map(|s| &**s)
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static SYMBOL_TABLE: RefCell<SymbolTable> = RefCell::new(SymbolTable::new());
}

/// Access the thread-local symbol table.
pub fn with_symbol_table<F, R>(f: F) -> R
where
    F: FnOnce(&mut SymbolTable) -> R,
{
    SYMBOL_TABLE.with(|cell| f(&mut cell.borrow_mut()))
}

/// Intern a symbol using the thread-local table.
pub fn intern(name: &str) -> InternedSymbol {
    with_symbol_table(|t| t.intern(name))
}

// ---------------------------------------------------------------------------
// Port types
// ---------------------------------------------------------------------------

/// I/O port for read/write operations.
pub enum Port {
    /// Input from a string.
    StringInput { data: String, pos: usize },
    /// Input from a bytevector (binary-safe).
    BytevectorInput { data: Vec<u8>, pos: usize },
    /// Output to a string buffer.
    StringOutput { buf: String },
    /// Output to a bytevector buffer (binary-safe).
    BytevectorOutput { buf: Vec<u8> },
    /// Input from a file.
    ///
    /// Text-mode ports lazily buffer all content into `text_buf` on first
    /// text read, then track position with `text_pos` — exactly like
    /// `StringInput`. This enables sequential `read`, `read-char`, and
    /// `peek-char` calls to share consistent position state.
    ///
    /// Binary-mode ports read directly from `reader`.
    FileInput {
        reader: Box<dyn std::io::Read>,
        name: String,
        binary: bool,
        /// Lazily populated text buffer for text-mode ports.
        text_buf: Option<String>,
        /// Current read position within `text_buf`.
        text_pos: usize,
    },
    /// Output to a file.
    FileOutput {
        writer: Box<dyn std::io::Write>,
        name: String,
        binary: bool,
    },
    /// Standard input.
    Stdin,
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
    /// Closed port — retains original kind for R7RS predicate semantics.
    /// R7RS §6.13.1: `input-port?` returns `#t` even on closed input ports.
    Closed(PortKind),
}

/// The kind of a port — preserved after close for predicate queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortKind {
    Input,
    Output,
}

impl Port {
    /// Ensure the text buffer is populated for text-mode FileInput ports.
    /// Reads all remaining content from the underlying reader into `text_buf`.
    /// Returns `Ok(())` on success, or an error if reading fails.
    /// No-op if already buffered or if this is a binary port.
    pub fn ensure_text_buffered(&mut self) -> Result<(), String> {
        if let Port::FileInput {
            reader,
            name,
            binary: false,
            text_buf,
            ..
        } = self
        {
            if text_buf.is_none() {
                use std::io::Read;
                let mut contents = String::new();
                reader
                    .read_to_string(&mut contents)
                    .map_err(|e| format!("error reading {name}: {e}"))?;
                *text_buf = Some(contents);
            }
        }
        Ok(())
    }

    /// Returns true if this port is open (not closed).
    pub fn is_open(&self) -> bool {
        !matches!(self, Port::Closed(_))
    }

    /// Returns true if this is an input port (open or closed).
    pub fn is_input(&self) -> bool {
        matches!(
            self,
            Port::StringInput { .. }
                | Port::BytevectorInput { .. }
                | Port::FileInput { .. }
                | Port::Stdin
                | Port::Closed(PortKind::Input)
        )
    }

    /// Returns true if this is an output port (open or closed).
    pub fn is_output(&self) -> bool {
        matches!(
            self,
            Port::StringOutput { .. }
                | Port::BytevectorOutput { .. }
                | Port::FileOutput { .. }
                | Port::Stdout
                | Port::Stderr
                | Port::Closed(PortKind::Output)
        )
    }

    /// Returns true if this is a binary port.
    pub fn is_binary(&self) -> bool {
        matches!(
            self,
            Port::FileInput { binary: true, .. }
                | Port::FileOutput { binary: true, .. }
                | Port::BytevectorInput { .. }
                | Port::BytevectorOutput { .. }
        )
    }

    /// The kind of this port (input or output).
    pub fn kind(&self) -> PortKind {
        match self {
            Port::StringInput { .. }
            | Port::BytevectorInput { .. }
            | Port::FileInput { .. }
            | Port::Stdin => PortKind::Input,
            Port::StringOutput { .. }
            | Port::BytevectorOutput { .. }
            | Port::FileOutput { .. }
            | Port::Stdout
            | Port::Stderr => PortKind::Output,
            Port::Closed(k) => *k,
        }
    }
}

impl fmt::Debug for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Port::StringInput { pos, data } => {
                write!(f, "StringInput(pos={}, len={})", pos, data.len())
            }
            Port::BytevectorInput { pos, data } => {
                write!(f, "BytevectorInput(pos={}, len={})", pos, data.len())
            }
            Port::StringOutput { buf } => write!(f, "StringOutput(len={})", buf.len()),
            Port::BytevectorOutput { buf } => write!(f, "BytevectorOutput(len={})", buf.len()),
            Port::FileInput { name, .. } => write!(f, "FileInput({name})"),
            Port::FileOutput { name, .. } => write!(f, "FileOutput({name})"),
            Port::Stdin => write!(f, "Stdin"),
            Port::Stdout => write!(f, "Stdout"),
            Port::Stderr => write!(f, "Stderr"),
            Port::Closed(k) => write!(f, "Closed({k:?})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Foreign function type
// ---------------------------------------------------------------------------

/// A Rust function callable from Scheme.
/// Returns Result for error propagation (solves Steel limitation #2).
pub struct ForeignFn {
    pub name: String,
    #[allow(clippy::type_complexity)]
    pub func: Box<dyn Fn(&[Value]) -> Result<Value, LispError>>,
    pub arity: Arity,
    pub doc: String,
}

impl fmt::Debug for ForeignFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<foreign {}>", self.name)
    }
}

// ---------------------------------------------------------------------------
// Closure and Continuation (forward-declared, filled in by compiler/vm)
// ---------------------------------------------------------------------------

/// Compiled closure: bytecode + captured environment.
#[derive(Clone, Debug)]
pub struct Closure {
    /// Index into the code pool.
    pub code_id: usize,
    /// Captured upvalues from enclosing scope (mutable cells for set! support).
    pub upvalues: Vec<Rc<RefCell<Value>>>,
    /// Arity for argument checking.
    pub arity: Arity,
    /// Name (for debugging/describe-function).
    pub name: Option<String>,
    /// Docstring (first string literal after define).
    pub doc: Option<String>,
}

/// A dynamic-wind extent entry.
/// Tracks the before/after thunks for dynamic-wind interaction with call/cc.
#[derive(Clone, Debug)]
pub struct Winder {
    /// The `before` thunk — called when entering this dynamic extent.
    pub before: Value,
    /// The `after` thunk — called when leaving this dynamic extent.
    pub after: Value,
}

/// Captured continuation for call/cc.
#[derive(Clone, Debug)]
pub struct Continuation {
    /// Snapshot of the value stack.
    pub stack: Vec<Value>,
    /// Snapshot of the call frames.
    pub frames: Vec<CallFrame>,
    /// Whether this continuation has been invoked.
    pub invoked: bool,
    /// Snapshot of the dynamic-wind stack at capture time.
    /// Used to compute which before/after thunks to run when
    /// this continuation is invoked.
    pub winders: Vec<Winder>,
}

/// A single call frame (activation record), captured by continuations.
#[derive(Clone, Debug)]
pub struct CallFrame {
    /// Index into the code pool.
    pub code_id: usize,
    /// Instruction pointer within the code.
    pub ip: usize,
    /// Base pointer into the value stack.
    pub bp: usize,
    /// Function name (for stack traces).
    pub function_name: Option<String>,
    /// Captured upvalues for this closure invocation.
    /// Shared cells so mutations through closures are visible across captures.
    pub upvalues: Vec<Rc<RefCell<Value>>>,
    /// Cells for locals that have been captured as upvalues.
    /// Shared cells ensure mutations are visible across continuation boundaries.
    pub local_cells: HashMap<usize, Rc<RefCell<Value>>>,
}

// ---------------------------------------------------------------------------
// Value enum
// ---------------------------------------------------------------------------

/// Tagged union — all Scheme values.
///
/// GC'd via Rc (Stage 1). The Trace trait is implemented so that
/// upgrading to gc-arena or mark-sweep requires only a backend swap.
#[derive(Clone, Debug)]
pub enum Value {
    /// The void/unspecified value.
    Void,
    /// Boolean #t or #f.
    Bool(bool),
    /// Exact integer (fixnum).
    Int(i64),
    /// Inexact real (flonum).
    Float(f64),
    /// Unicode character.
    Char(char),
    /// Immutable string.
    String(Rc<str>),
    /// Interned symbol.
    Symbol(InternedSymbol),
    /// Cons cell (pair).
    Pair(Rc<(Value, Value)>),
    /// Mutable vector.
    Vector(Rc<RefCell<Vec<Value>>>),
    /// Mutable bytevector.
    Bytevector(Rc<RefCell<Vec<u8>>>),
    /// Compiled closure (lambda + captured env).
    Closure(Rc<Closure>),
    /// Captured continuation (call/cc).
    Continuation(Rc<Continuation>),
    /// I/O port.
    Port(Rc<RefCell<Port>>),
    /// Rust foreign function.
    Foreign(Rc<ForeignFn>),
    /// Uninitialized binding.
    Undefined,
    /// End of file object.
    Eof,
    /// Null (empty list).
    Null,
}

impl Value {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    pub fn string(s: impl Into<String>) -> Self {
        Value::String(Rc::from(s.into().as_str()))
    }

    pub fn symbol(name: &str) -> Self {
        Value::Symbol(intern(name))
    }

    pub fn cons(car: Value, cdr: Value) -> Self {
        Value::Pair(Rc::new((car, cdr)))
    }

    /// Build a proper list from an iterator of values.
    pub fn list(values: impl IntoIterator<Item = Value>) -> Self {
        let items: Vec<Value> = values.into_iter().collect();
        let mut result = Value::Null;
        for v in items.into_iter().rev() {
            result = Value::cons(v, result);
        }
        result
    }

    pub fn vector(values: Vec<Value>) -> Self {
        Value::Vector(Rc::new(RefCell::new(values)))
    }

    pub fn bytevector(bytes: Vec<u8>) -> Self {
        Value::Bytevector(Rc::new(RefCell::new(bytes)))
    }

    // -----------------------------------------------------------------------
    // Predicates
    // -----------------------------------------------------------------------

    pub fn is_true(&self) -> bool {
        !matches!(self, Value::Bool(false))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn is_pair(&self) -> bool {
        matches!(self, Value::Pair(_))
    }

    pub fn is_list(&self) -> bool {
        let mut cur = self.clone();
        loop {
            match cur {
                Value::Null => return true,
                Value::Pair(p) => cur = p.1.clone(),
                _ => return false,
            }
        }
    }

    /// Convert a Scheme list to a Vec of Values. Returns None for non-lists.
    pub fn to_list(&self) -> Option<Vec<Value>> {
        let mut result = Vec::new();
        let mut cur = self.clone();
        loop {
            match cur {
                Value::Null => return Some(result),
                Value::Pair(p) => {
                    result.push(p.0.clone());
                    cur = p.1.clone();
                }
                _ => return None,
            }
        }
    }

    pub fn is_number(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_))
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::String(_))
    }

    pub fn is_symbol(&self) -> bool {
        matches!(self, Value::Symbol(_))
    }

    pub fn is_procedure(&self) -> bool {
        matches!(
            self,
            Value::Closure(_) | Value::Foreign(_) | Value::Continuation(_)
        )
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn as_int(&self) -> Result<i64, LispError> {
        match self {
            Value::Int(n) => Ok(*n),
            _ => Err(LispError::type_error("integer", self.type_name())),
        }
    }

    pub fn as_float(&self) -> Result<f64, LispError> {
        match self {
            Value::Float(n) => Ok(*n),
            Value::Int(n) => Ok(*n as f64),
            _ => Err(LispError::type_error("number", self.type_name())),
        }
    }

    pub fn as_str(&self) -> Result<&str, LispError> {
        match self {
            Value::String(s) => Ok(s),
            _ => Err(LispError::type_error("string", self.type_name())),
        }
    }

    pub fn as_symbol(&self) -> Result<&InternedSymbol, LispError> {
        match self {
            Value::Symbol(s) => Ok(s),
            _ => Err(LispError::type_error("symbol", self.type_name())),
        }
    }

    pub fn as_char(&self) -> Result<char, LispError> {
        match self {
            Value::Char(c) => Ok(*c),
            _ => Err(LispError::type_error("char", self.type_name())),
        }
    }

    pub fn as_bool(&self) -> Result<bool, LispError> {
        match self {
            Value::Bool(b) => Ok(*b),
            _ => Err(LispError::type_error("boolean", self.type_name())),
        }
    }

    pub fn car(&self) -> Result<Value, LispError> {
        match self {
            Value::Pair(p) => Ok(p.0.clone()),
            _ => Err(LispError::type_error("pair", self.type_name())),
        }
    }

    pub fn cdr(&self) -> Result<Value, LispError> {
        match self {
            Value::Pair(p) => Ok(p.1.clone()),
            _ => Err(LispError::type_error("pair", self.type_name())),
        }
    }

    /// Convert a proper list to a Vec.
    pub fn to_vec(&self) -> Result<Vec<Value>, LispError> {
        let mut result = Vec::new();
        let mut cur = self.clone();
        loop {
            match cur {
                Value::Null => return Ok(result),
                Value::Pair(p) => {
                    result.push(p.0.clone());
                    cur = p.1.clone();
                }
                _ => return Err(LispError::type_error("proper list", self.type_name())),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Type name for error messages
    // -----------------------------------------------------------------------

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Void => "void",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "float",
            Value::Char(_) => "char",
            Value::String(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::Pair(_) => "pair",
            Value::Vector(_) => "vector",
            Value::Bytevector(_) => "bytevector",
            Value::Closure(_) => "procedure",
            Value::Continuation(_) => "continuation",
            Value::Port(_) => "port",
            Value::Foreign(_) => "procedure",
            Value::Undefined => "undefined",
            Value::Eof => "eof",
            Value::Null => "null",
        }
    }

    // -----------------------------------------------------------------------
    // Equivalence (R7RS §6.1)
    // -----------------------------------------------------------------------

    /// R7RS `eq?` — identity comparison.
    pub fn is_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Void, Value::Void) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Eof, Value::Eof) => true,
            (Value::Undefined, Value::Undefined) => true,
            (Value::Pair(a), Value::Pair(b)) => Rc::ptr_eq(a, b),
            (Value::Vector(a), Value::Vector(b)) => Rc::ptr_eq(a, b),
            (Value::String(a), Value::String(b)) => std::ptr::eq(a.as_ptr(), b.as_ptr()),
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::Foreign(a), Value::Foreign(b)) => Rc::ptr_eq(a, b),
            (Value::Continuation(a), Value::Continuation(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// R7RS `eqv?` — like eq? but compares floats by value.
    pub fn is_eqv(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Float(a), Value::Float(b)) => a == b,
            _ => self.is_eq(other),
        }
    }

    /// R7RS `equal?` — recursive structural equality.
    pub fn is_equal(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Pair(a), Value::Pair(b)) => a.0.is_equal(&b.0) && a.1.is_equal(&b.1),
            (Value::Vector(a), Value::Vector(b)) => {
                let a = a.borrow();
                let b = b.borrow();
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.is_equal(y))
            }
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Bytevector(a), Value::Bytevector(b)) => {
                a.borrow().as_slice() == b.borrow().as_slice()
            }
            _ => self.is_eqv(other),
        }
    }

    /// Check if this is an exact number (integer).
    pub fn is_exact(&self) -> bool {
        matches!(self, Value::Int(_))
    }

    /// Returns true only for `#f`.
    pub fn is_false(&self) -> bool {
        matches!(self, Value::Bool(false))
    }

    /// Try to get a float value (returns Option for convenience in stdlib).
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(n) => Some(*n as f64),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// PartialEq — Scheme eqv? semantics
// ---------------------------------------------------------------------------

impl PartialEq for Value {
    /// Structural equality (R7RS `equal?` semantics).
    ///
    /// Pairs and vectors are compared recursively by structure, not by identity.
    /// This matches R7RS §6.1: `equal?` recursively compares pairs, vectors,
    /// strings, and bytevectors. Closures and ports use identity (Rc pointer).
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Void, Value::Void) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::Null, Value::Null) => true,
            (Value::Eof, Value::Eof) => true,
            (Value::Undefined, Value::Undefined) => true,
            // Pairs: structural comparison (recursive)
            (Value::Pair(a), Value::Pair(b)) => Rc::ptr_eq(a, b) || (a.0 == b.0 && a.1 == b.1),
            // Vectors: structural comparison (element-wise)
            (Value::Vector(a), Value::Vector(b)) => {
                Rc::ptr_eq(a, b) || a.borrow().as_slice() == b.borrow().as_slice()
            }
            // Bytevectors: structural comparison
            (Value::Bytevector(a), Value::Bytevector(b)) => {
                Rc::ptr_eq(a, b) || a.borrow().as_slice() == b.borrow().as_slice()
            }
            // Closures, ports: identity comparison
            (Value::Closure(a), Value::Closure(b)) => Rc::ptr_eq(a, b),
            (Value::Foreign(a), Value::Foreign(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Display — Scheme write semantics
// ---------------------------------------------------------------------------

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Void => write!(f, "#<void>"),
            Value::Bool(true) => write!(f, "#t"),
            Value::Bool(false) => write!(f, "#f"),
            Value::Int(n) => write!(f, "{n}"),
            Value::Float(n) => {
                if n.is_nan() {
                    write!(f, "+nan.0")
                } else if n.is_infinite() {
                    if *n > 0.0 {
                        write!(f, "+inf.0")
                    } else {
                        write!(f, "-inf.0")
                    }
                } else if n.fract() == 0.0 {
                    write!(f, "{n:.1}")
                } else {
                    write!(f, "{n}")
                }
            }
            Value::Char(c) => write_char(f, *c),
            Value::String(s) => write_string(f, s),
            Value::Symbol(s) => write!(f, "{}", s.name()),
            Value::Pair(p) => write_pair(f, &p.0, &p.1),
            Value::Vector(v) => {
                write!(f, "#(")?;
                let v = v.borrow();
                for (i, val) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{val}")?;
                }
                write!(f, ")")
            }
            Value::Bytevector(bv) => {
                write!(f, "#u8(")?;
                let bv = bv.borrow();
                for (i, b) in bv.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{b}")?;
                }
                write!(f, ")")
            }
            Value::Closure(c) => {
                if let Some(name) = &c.name {
                    write!(f, "#<procedure {name}>")
                } else {
                    write!(f, "#<procedure>")
                }
            }
            Value::Continuation(_) => write!(f, "#<continuation>"),
            Value::Port(p) => {
                let p = p.borrow();
                write!(f, "#<port {:?}>", *p)
            }
            Value::Foreign(ff) => write!(f, "#<procedure {}>", ff.name),
            Value::Undefined => write!(f, "#<undefined>"),
            Value::Eof => write!(f, "#<eof>"),
            Value::Null => write!(f, "()"),
        }
    }
}

/// Display variant for Scheme `display` (no quotes on strings, chars as-is).
/// Format a value for `display` (R7RS §6.13.3).
///
/// Unlike `write` (the Display trait), `display` omits quotes on strings,
/// renders characters as their character (not `#\x`), and recurses into
/// lists and vectors with `display` semantics on each element.
pub fn display_value(val: &Value) -> String {
    match val {
        Value::String(s) => s.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Pair(p) => {
            let mut out = String::from("(");
            out.push_str(&display_value(&p.0));
            let mut current = &p.1;
            loop {
                match current {
                    Value::Null => break,
                    Value::Pair(p2) => {
                        out.push(' ');
                        out.push_str(&display_value(&p2.0));
                        current = &p2.1;
                    }
                    other => {
                        out.push_str(" . ");
                        out.push_str(&display_value(other));
                        break;
                    }
                }
            }
            out.push(')');
            out
        }
        Value::Vector(v) => {
            let v = v.borrow();
            let mut out = String::from("#(");
            for (i, elem) in v.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&display_value(elem));
            }
            out.push(')');
            out
        }
        _ => format!("{val}"),
    }
}

fn write_char(f: &mut fmt::Formatter<'_>, c: char) -> fmt::Result {
    match c {
        ' ' => write!(f, "#\\space"),
        '\n' => write!(f, "#\\newline"),
        '\r' => write!(f, "#\\return"),
        '\t' => write!(f, "#\\tab"),
        '\0' => write!(f, "#\\null"),
        '\x07' => write!(f, "#\\alarm"),
        '\x08' => write!(f, "#\\backspace"),
        '\x1b' => write!(f, "#\\escape"),
        '\x7f' => write!(f, "#\\delete"),
        c if c.is_ascii_graphic() => write!(f, "#\\{c}"),
        c => write!(f, "#\\x{:x}", c as u32),
    }
}

fn write_string(f: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    write!(f, "\"")?;
    for c in s.chars() {
        match c {
            '"' => write!(f, "\\\"")?,
            '\\' => write!(f, "\\\\")?,
            '\n' => write!(f, "\\n")?,
            '\r' => write!(f, "\\r")?,
            '\t' => write!(f, "\\t")?,
            '\x07' => write!(f, "\\a")?,
            '\x08' => write!(f, "\\b")?,
            c if c.is_control() => write!(f, "\\x{:x};", c as u32)?,
            c => write!(f, "{c}")?,
        }
    }
    write!(f, "\"")
}

fn write_pair(f: &mut fmt::Formatter<'_>, car: &Value, cdr: &Value) -> fmt::Result {
    write!(f, "({car}")?;
    let mut current = cdr.clone();
    loop {
        match current {
            Value::Null => break,
            Value::Pair(p) => {
                write!(f, " {}", p.0)?;
                current = p.1.clone();
            }
            other => {
                write!(f, " . {other}")?;
                break;
            }
        }
    }
    write!(f, ")")
}

// ---------------------------------------------------------------------------
// Trace implementation (Stage 1: traversal for future GC)
// ---------------------------------------------------------------------------

impl Trace for Value {
    fn trace(&self, tracer: &mut dyn Tracer) {
        match self {
            Value::Pair(p) => {
                tracer.trace_value(&p.0);
                tracer.trace_value(&p.1);
            }
            Value::Vector(v) => {
                for val in v.borrow().iter() {
                    tracer.trace_value(val);
                }
            }
            Value::Closure(c) => {
                for cell in &c.upvalues {
                    tracer.trace_value(&cell.borrow());
                }
            }
            Value::Continuation(cont) => {
                for val in &cont.stack {
                    tracer.trace_value(val);
                }
                // Trace captured frames (upvalues + local_cells hold live values)
                for frame in &cont.frames {
                    for cell in &frame.upvalues {
                        tracer.trace_value(&cell.borrow());
                    }
                    for cell in frame.local_cells.values() {
                        tracer.trace_value(&cell.borrow());
                    }
                }
                // Trace winder thunks
                for w in &cont.winders {
                    tracer.trace_value(&w.before);
                    tracer.trace_value(&w.after);
                }
            }
            // Atoms and leaf types: nothing to trace
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_interning() {
        let a = intern("foo");
        let b = intern("foo");
        let c = intern("bar");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.name(), "foo");
    }

    #[test]
    fn test_value_constructors() {
        let v = Value::Int(42);
        assert_eq!(v.as_int().unwrap(), 42);

        let s = Value::string("hello");
        assert_eq!(s.as_str().unwrap(), "hello");

        let sym = Value::symbol("test");
        assert!(sym.is_symbol());
    }

    #[test]
    fn test_list_construction() {
        let list = Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert!(list.is_list());
        let vec = list.to_vec().unwrap();
        assert_eq!(vec.len(), 3);
        assert_eq!(vec[0].as_int().unwrap(), 1);
        assert_eq!(vec[2].as_int().unwrap(), 3);
    }

    #[test]
    fn test_null_is_list() {
        assert!(Value::Null.is_list());
        assert!(Value::Null.is_null());
    }

    #[test]
    fn test_dotted_pair_not_list() {
        let pair = Value::cons(Value::Int(1), Value::Int(2));
        assert!(pair.is_pair());
        assert!(!pair.is_list());
    }

    #[test]
    fn test_display_atoms() {
        assert_eq!(format!("{}", Value::Bool(true)), "#t");
        assert_eq!(format!("{}", Value::Bool(false)), "#f");
        assert_eq!(format!("{}", Value::Int(42)), "42");
        assert_eq!(format!("{}", Value::Float(2.75)), "2.75");
        assert_eq!(format!("{}", Value::Float(1.0)), "1.0");
        assert_eq!(format!("{}", Value::Null), "()");
        assert_eq!(format!("{}", Value::Void), "#<void>");
    }

    #[test]
    fn test_display_string() {
        let s = Value::string("hello\nworld");
        assert_eq!(format!("{s}"), "\"hello\\nworld\"");
    }

    #[test]
    fn test_display_char() {
        assert_eq!(format!("{}", Value::Char('a')), "#\\a");
        assert_eq!(format!("{}", Value::Char(' ')), "#\\space");
        assert_eq!(format!("{}", Value::Char('\n')), "#\\newline");
        assert_eq!(format!("{}", Value::Char('\t')), "#\\tab");
    }

    #[test]
    fn test_display_list() {
        let list = Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        assert_eq!(format!("{list}"), "(1 2 3)");
    }

    #[test]
    fn test_display_dotted_pair() {
        let pair = Value::cons(Value::Int(1), Value::Int(2));
        assert_eq!(format!("{pair}"), "(1 . 2)");
    }

    #[test]
    fn test_display_nested_list() {
        let inner = Value::list(vec![Value::Int(2), Value::Int(3)]);
        let outer = Value::list(vec![Value::Int(1), inner]);
        assert_eq!(format!("{outer}"), "(1 (2 3))");
    }

    #[test]
    fn test_display_vector() {
        let v = Value::vector(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(format!("{v}"), "#(1 2)");
    }

    #[test]
    fn test_display_bytevector() {
        let bv = Value::bytevector(vec![1, 2, 3]);
        assert_eq!(format!("{bv}"), "#u8(1 2 3)");
    }

    #[test]
    fn test_eq_semantics() {
        // Same-value atoms are eq
        assert_eq!(Value::Int(1), Value::Int(1));
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_eq!(Value::string("a"), Value::string("a"));
        assert_eq!(Value::Null, Value::Null);

        // Different-value atoms are not eq
        assert_ne!(Value::Int(1), Value::Int(2));

        // Pairs: structural equality (R7RS equal? semantics)
        let p1 = Value::cons(Value::Int(1), Value::Null);
        let p2 = Value::cons(Value::Int(1), Value::Null);
        assert_eq!(p1, p2); // same structure
        assert_eq!(p1, p1.clone()); // same Rc (fast path)

        // Identity (eq?) uses is_eq, not PartialEq
        assert!(!p1.is_eq(&p2)); // different Rc pointers
        assert!(p1.is_eq(&p1.clone())); // same Rc
    }

    #[test]
    fn test_type_errors() {
        let v = Value::string("hello");
        assert!(v.as_int().is_err());
        let err = v.as_int().unwrap_err();
        assert!(err.message().contains("expected integer"));
        assert!(err.message().contains("string"));
    }

    #[test]
    fn test_is_true() {
        assert!(Value::Int(0).is_true());
        assert!(Value::string("").is_true());
        assert!(Value::Null.is_true());
        assert!(Value::Bool(true).is_true());
        assert!(!Value::Bool(false).is_true());
    }

    #[test]
    fn test_display_value() {
        assert_eq!(display_value(&Value::string("hello")), "hello");
        assert_eq!(display_value(&Value::Char('a')), "a");
        assert_eq!(display_value(&Value::Int(42)), "42");
    }

    #[test]
    fn test_trace_traversal() {
        // Verify Trace finds all children
        struct Counter {
            count: usize,
        }
        impl Tracer for Counter {
            fn trace_value(&mut self, _: &Value) {
                self.count += 1;
            }
        }

        let list = Value::list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let mut counter = Counter { count: 0 };
        list.trace(&mut counter);
        // Pair traces car + cdr; outermost pair traces Int(1) + (2 3)
        assert_eq!(counter.count, 2);

        let vec = Value::vector(vec![Value::Int(1), Value::Int(2)]);
        let mut counter = Counter { count: 0 };
        vec.trace(&mut counter);
        assert_eq!(counter.count, 2);
    }
}
