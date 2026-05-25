//! R7RS §6.13: I/O and display primitives.
//!
//! ## mae-scheme I/O stance
//!
//! ### Port model
//! Ports are enum variants: `StringInput`, `StringOutput`, `FileInput`,
//! `FileOutput`, `Stdin`, `Stdout`, `Stderr`, `Closed`. Operations on
//! closed ports signal errors (R7RS §6.13.1).
//!
//! ### Current ports
//! `current-input-port`, `current-output-port`, `current-error-port` return
//! the process-level stdin/stdout/stderr. They are NOT dynamically
//! parameterizable via `parameterize` (planned). `with-input-from-file`
//! and `with-output-to-file` are simplified (see SPEC_STANCES.md §8).
//!
//! ### Binary I/O
//! `read-u8`, `peek-u8`, `write-u8`, `read-bytevector`, `write-bytevector`
//! operate on bytevectors. `binary-port?` returns `#f` for text ports (all
//! file ports are opened in text mode by default).
//!
//! ### String ports
//! `open-input-string` and `open-output-string` / `get-output-string` provide
//! in-memory I/O. These are the most commonly used port types in extension code.

use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::reader::Reader;
use crate::value::{display_value, Port, Value};
use crate::vm::Vm;

/// Determine width of UTF-8 character from its first byte.
fn utf8_char_width(first: u8) -> usize {
    match first {
        0..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Write a string to a port value.
fn write_to_port(port_val: &Value, text: &str) -> Result<(), LispError> {
    match port_val {
        Value::Port(port_cell) => {
            let mut port = port_cell.borrow_mut();
            match &mut *port {
                Port::Closed => Err(LispError::user("write: port is closed", vec![])),
                Port::StringOutput { buf } => {
                    buf.push_str(text);
                    Ok(())
                }
                Port::Stdout => {
                    print!("{text}");
                    Ok(())
                }
                Port::Stderr => {
                    eprint!("{text}");
                    Ok(())
                }
                Port::FileOutput { writer, .. } => {
                    use std::io::Write;
                    writer
                        .write_all(text.as_bytes())
                        .map_err(|e| LispError::internal(format!("write error: {e}")))?;
                    Ok(())
                }
                _ => Err(LispError::type_error("output-port", "input-port")),
            }
        }
        _ => Err(LispError::type_error("port", port_val.type_name())),
    }
}

pub fn register(vm: &mut Vm) {
    vm.register_fn(
        "display",
        "Display value (human-readable, no quotes on strings)",
        Arity::Variadic(1),
        |args| {
            let text = display_value(&args[0]);
            if args.len() > 1 {
                // Write to port
                write_to_port(&args[1], &text)?;
            } else {
                print!("{text}");
            }
            Ok(Value::Void)
        },
    );

    vm.register_fn(
        "write",
        "Write value (machine-readable, with quotes)",
        Arity::Variadic(1),
        |args| {
            let text = format!("{}", args[0]);
            if args.len() > 1 {
                write_to_port(&args[1], &text)?;
            } else {
                print!("{text}");
            }
            Ok(Value::Void)
        },
    );

    // R7RS §6.13.2 read — read one S-expression from port
    vm.register_fn(
        "read",
        "Read one S-expression from port",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                // No port — reading from stdin not supported in this context
                return Err(LispError::user(
                    "read: no current-input-port (pass a port argument)",
                    vec![],
                ));
            }
            match &args[0] {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed => Err(LispError::user("read: port is closed", vec![])),
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                return Ok(Value::Eof);
                            }
                            let remaining = &data[*pos..];
                            let mut reader = Reader::new(remaining, "<read>");
                            match reader.read() {
                                Ok(Some(val)) => {
                                    *pos += reader.position();
                                    Ok(val)
                                }
                                Ok(None) => Ok(Value::Eof),
                                Err(e) => Err(e),
                            }
                        }
                        Port::FileInput {
                            reader: file_reader,
                            name,
                        } => {
                            use std::io::Read;
                            let mut contents = String::new();
                            file_reader.read_to_string(&mut contents).map_err(|e| {
                                LispError::internal(format!("read: error reading {}: {e}", name))
                            })?;
                            if contents.is_empty() {
                                return Ok(Value::Eof);
                            }
                            let mut reader = Reader::new(&contents, name.as_str());
                            match reader.read() {
                                Ok(Some(val)) => Ok(val),
                                Ok(None) => Ok(Value::Eof),
                                Err(e) => Err(e),
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "output-port")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    vm.register_fn("newline", "Print newline", Arity::Variadic(0), |args| {
        if !args.is_empty() {
            write_to_port(&args[0], "\n")?;
        } else {
            println!();
        }
        Ok(Value::Void)
    });

    // String output
    vm.register_fn(
        "display-string",
        "Display a string (no quotes)",
        Arity::Fixed(1),
        |args| {
            print!("{}", args[0].as_str()?);
            Ok(Value::Void)
        },
    );

    // String ports (in-memory I/O)
    vm.register_fn(
        "open-input-string",
        "Create input port from string",
        Arity::Fixed(1),
        |args| {
            let s = args[0].as_str()?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                crate::value::Port::StringInput {
                    data: s.to_string(),
                    pos: 0,
                },
            ))))
        },
    );

    vm.register_fn(
        "open-output-string",
        "Create output string port",
        Arity::Fixed(0),
        |_args| {
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                crate::value::Port::StringOutput { buf: String::new() },
            ))))
        },
    );

    vm.register_fn(
        "get-output-string",
        "Get accumulated string from output port",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => {
                let port = p.borrow();
                match &*port {
                    crate::value::Port::StringOutput { buf: data } => {
                        Ok(Value::String(Rc::from(data.as_str())))
                    }
                    _ => Err(LispError::type_error(
                        "output-string-port",
                        "other port type",
                    )),
                }
            }
            _ => Err(LispError::type_error("port", format!("{}", args[0]))),
        },
    );

    // read-char from string port
    vm.register_fn(
        "read-char",
        "Read a character from port",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                return Ok(Value::Eof);
            }
            match &args[0] {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed => Err(LispError::user("read-char: port is closed", vec![])),
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = data[*pos..].chars().next().unwrap();
                                *pos += ch.len_utf8();
                                Ok(Value::Char(ch))
                            }
                        }
                        Port::FileInput { reader, .. } => {
                            use std::io::Read;
                            let mut buf = [0u8; 4];
                            match reader.read(&mut buf[..1]) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(_) => {
                                    // Handle UTF-8 multi-byte
                                    let needed = utf8_char_width(buf[0]);
                                    if needed > 1 {
                                        let _ = reader.read_exact(&mut buf[1..needed]);
                                    }
                                    let s =
                                        std::str::from_utf8(&buf[..needed]).unwrap_or("\u{FFFD}");
                                    Ok(Value::Char(s.chars().next().unwrap_or('\u{FFFD}')))
                                }
                                Err(_) => Ok(Value::Eof),
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    // peek-char
    vm.register_fn(
        "peek-char",
        "Peek at next character from port",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                return Ok(Value::Eof);
            }
            match &args[0] {
                Value::Port(p) => {
                    let port = p.borrow();
                    match &*port {
                        Port::Closed => Err(LispError::user("peek-char: port is closed", vec![])),
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = data[*pos..].chars().next().unwrap();
                                Ok(Value::Char(ch))
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    // write-string to port
    vm.register_fn(
        "write-string",
        "Write string to port",
        Arity::Variadic(1),
        |args| {
            let s = args[0].as_str()?;
            if args.len() > 1 {
                write_to_port(&args[1], s)?;
            } else {
                print!("{s}");
            }
            Ok(Value::Void)
        },
    );

    // Port predicates
    vm.register_fn(
        "input-port?",
        "Is input port?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => {
                let port = p.borrow();
                Ok(Value::Bool(matches!(
                    &*port,
                    crate::value::Port::StringInput { .. }
                        | crate::value::Port::Stdin
                        | crate::value::Port::FileInput { .. }
                )))
            }
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "output-port?",
        "Is output port?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => {
                let port = p.borrow();
                Ok(Value::Bool(matches!(
                    &*port,
                    crate::value::Port::StringOutput { .. }
                        | crate::value::Port::Stdout
                        | crate::value::Port::Stderr
                        | crate::value::Port::FileOutput { .. }
                )))
            }
            _ => Ok(Value::Bool(false)),
        },
    );

    // EOF object
    vm.register_fn(
        "eof-object",
        "Return the EOF object",
        Arity::Fixed(0),
        |_args| Ok(Value::Eof),
    );

    // with-output-to-string (convenience, not R7RS but very useful)
    vm.register_fn(
        "format",
        "Simple string formatting: (format \"~a is ~a\" x y)",
        Arity::Variadic(1),
        |args| {
            let template = args[0].as_str()?;
            let mut result = String::new();
            let mut arg_idx = 1;
            let mut chars = template.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '~' {
                    if let Some(&spec) = chars.peek() {
                        chars.next();
                        match spec {
                            'a' | 'A' => {
                                if arg_idx < args.len() {
                                    result.push_str(&display_value(&args[arg_idx]));
                                    arg_idx += 1;
                                }
                            }
                            's' | 'S' => {
                                if arg_idx < args.len() {
                                    result.push_str(&format!("{}", args[arg_idx]));
                                    arg_idx += 1;
                                }
                            }
                            '%' => result.push('\n'),
                            '~' => result.push('~'),
                            _ => {
                                result.push('~');
                                result.push(spec);
                            }
                        }
                    }
                } else {
                    result.push(c);
                }
            }
            Ok(Value::String(Rc::from(result.as_str())))
        },
    );

    // R7RS §6.13.1 Port predicates and standard ports
    vm.register_fn(
        "textual-port?",
        "Is textual port?",
        Arity::Fixed(1),
        |args| Ok(Value::Bool(matches!(args[0], Value::Port(_)))),
    );

    vm.register_fn(
        "binary-port?",
        "Is binary port?",
        Arity::Fixed(1),
        |_args| Ok(Value::Bool(false)), // We only have textual ports
    );

    vm.register_fn(
        "input-port-open?",
        "Is input port open?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => {
                let port = p.borrow();
                Ok(Value::Bool(port.is_input() && port.is_open()))
            }
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "output-port-open?",
        "Is output port open?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => {
                let port = p.borrow();
                Ok(Value::Bool(port.is_output() && port.is_open()))
            }
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn("close-port", "Close a port", Arity::Fixed(1), |args| {
        if let Value::Port(p) = &args[0] {
            *p.borrow_mut() = Port::Closed;
        }
        Ok(Value::Void)
    });

    vm.register_fn(
        "close-input-port",
        "Close input port",
        Arity::Fixed(1),
        |args| {
            if let Value::Port(p) = &args[0] {
                *p.borrow_mut() = Port::Closed;
            }
            Ok(Value::Void)
        },
    );

    vm.register_fn(
        "close-output-port",
        "Close output port",
        Arity::Fixed(1),
        |args| {
            if let Value::Port(p) = &args[0] {
                *p.borrow_mut() = Port::Closed;
            }
            Ok(Value::Void)
        },
    );

    vm.register_fn(
        "flush-output-port",
        "Flush output port",
        Arity::Variadic(0),
        |_args| Ok(Value::Void), // No-op for string ports
    );

    // read-line from input port
    vm.register_fn(
        "read-line",
        "Read a line from input port",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                return Err(LispError::user("read-line: no current-input-port", vec![]));
            }
            match &args[0] {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed => Err(LispError::user("read-line: port is closed", vec![])),
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                return Ok(Value::Eof);
                            }
                            let remaining = &data[*pos..];
                            if let Some(nl) = remaining.find('\n') {
                                let line = &remaining[..nl];
                                *pos += nl + 1;
                                Ok(Value::String(Rc::from(line)))
                            } else {
                                let line = remaining.to_string();
                                *pos = data.len();
                                Ok(Value::String(Rc::from(line.as_str())))
                            }
                        }
                        Port::FileInput { reader, .. } => {
                            use std::io::BufRead;
                            let mut line = String::new();
                            let reader: &mut dyn std::io::Read = &mut **reader;
                            let mut buf_reader = std::io::BufReader::new(reader);
                            match buf_reader.read_line(&mut line) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(_) => {
                                    // Strip trailing newline
                                    if line.ends_with('\n') {
                                        line.pop();
                                        if line.ends_with('\r') {
                                            line.pop();
                                        }
                                    }
                                    Ok(Value::String(Rc::from(line.as_str())))
                                }
                                Err(_) => Ok(Value::Eof),
                            }
                        }
                        _ => Err(LispError::type_error("input port", "output port")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    // R7RS §6.14 features
    vm.register_fn(
        "features",
        "Implementation features",
        Arity::Fixed(0),
        |_| {
            Ok(Value::list(vec![
                Value::symbol("r7rs"),
                Value::symbol("mae"),
                Value::symbol("mae-scheme"),
                Value::symbol("ratios"),
                Value::symbol("exact-complex"),
            ]))
        },
    );

    // R7RS §6.13.1 Standard ports
    vm.register_fn(
        "current-input-port",
        "Current default input port",
        Arity::Fixed(0),
        |_args| Ok(Value::Port(Rc::new(std::cell::RefCell::new(Port::Stdin)))),
    );

    vm.register_fn(
        "current-output-port",
        "Current default output port",
        Arity::Fixed(0),
        |_args| Ok(Value::Port(Rc::new(std::cell::RefCell::new(Port::Stdout)))),
    );

    vm.register_fn(
        "current-error-port",
        "Current default error port",
        Arity::Fixed(0),
        |_args| Ok(Value::Port(Rc::new(std::cell::RefCell::new(Port::Stderr)))),
    );

    // R7RS §6.13.3 Binary I/O — bytevector ports
    vm.register_fn(
        "open-input-bytevector",
        "Create input port from bytevector",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                // Convert bytes to string for our StringInput port
                let bytes = bv.borrow().clone();
                let data = bytes.iter().map(|b| *b as char).collect::<String>();
                Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                    Port::StringInput { data, pos: 0 },
                ))))
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "open-output-bytevector",
        "Create output bytevector port",
        Arity::Fixed(0),
        |_args| {
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::StringOutput { buf: String::new() },
            ))))
        },
    );

    vm.register_fn(
        "get-output-bytevector",
        "Get accumulated bytes from output bytevector port",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => {
                let port = p.borrow();
                match &*port {
                    Port::StringOutput { buf } => {
                        let bytes: Vec<u8> = buf.bytes().collect();
                        Ok(Value::bytevector(bytes))
                    }
                    _ => Err(LispError::type_error(
                        "output-bytevector-port",
                        "other port type",
                    )),
                }
            }
            _ => Err(LispError::type_error("port", format!("{}", args[0]))),
        },
    );

    // R7RS §6.13.3 read-u8, peek-u8, write-u8
    vm.register_fn(
        "read-u8",
        "Read a byte from port",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                return Ok(Value::Eof);
            }
            match &args[0] {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let byte = data.as_bytes()[*pos];
                                *pos += 1;
                                Ok(Value::Int(byte as i64))
                            }
                        }
                        Port::FileInput { reader, .. } => {
                            use std::io::Read;
                            let mut buf = [0u8; 1];
                            match reader.read(&mut buf) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(_) => Ok(Value::Int(buf[0] as i64)),
                                Err(e) => Err(LispError::user(format!("read-u8: {e}"), vec![])),
                            }
                        }
                        Port::Closed => Err(LispError::user("read-u8: port is closed", vec![])),
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    vm.register_fn(
        "peek-u8",
        "Peek at next byte from port",
        Arity::Variadic(0),
        |args| {
            if args.is_empty() {
                return Ok(Value::Eof);
            }
            match &args[0] {
                Value::Port(p) => {
                    let port = p.borrow();
                    match &*port {
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                Ok(Value::Int(data.as_bytes()[*pos] as i64))
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    vm.register_fn(
        "write-u8",
        "Write a byte to port",
        Arity::Variadic(1),
        |args| {
            let byte = args[0].as_int()? as u8;
            if args.len() > 1 {
                write_to_port(&args[1], &String::from(byte as char))?;
            } else {
                print!("{}", byte as char);
            }
            Ok(Value::Void)
        },
    );

    // R7RS §6.13.3 read-bytevector, write-bytevector
    vm.register_fn(
        "read-bytevector",
        "Read k bytes from port",
        Arity::Variadic(1),
        |args| {
            let k = args[0].as_int()? as usize;
            if args.len() < 2 {
                return Ok(Value::Eof);
            }
            match &args[1] {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                return Ok(Value::Eof);
                            }
                            let end = (*pos + k).min(data.len());
                            let bytes: Vec<u8> = data.as_bytes()[*pos..end].to_vec();
                            *pos = end;
                            Ok(Value::bytevector(bytes))
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[1]))),
            }
        },
    );

    vm.register_fn(
        "write-bytevector",
        "Write bytevector to port. Optional start/end select a range.",
        Arity::Variadic(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let bytes = bv.borrow();
                let start = if args.len() > 2 {
                    args[2].as_int()? as usize
                } else {
                    0
                };
                let end = if args.len() > 3 {
                    args[3].as_int()? as usize
                } else {
                    bytes.len()
                };
                let slice = &bytes[start..end];
                let text: String = slice.iter().map(|b| *b as char).collect();
                if args.len() > 1 {
                    write_to_port(&args[1], &text)?;
                } else {
                    print!("{text}");
                }
                Ok(Value::Void)
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    // R7RS char-ready? and u8-ready?
    vm.register_fn(
        "char-ready?",
        "Is character ready on port?",
        Arity::Variadic(0),
        |_args| Ok(Value::Bool(true)), // Always ready for string ports
    );

    vm.register_fn(
        "u8-ready?",
        "Is byte ready on port?",
        Arity::Variadic(0),
        |_args| Ok(Value::Bool(true)),
    );

    // R7RS §6.13.2 write-char with port support (override Fixed(1) version)
    vm.register_fn(
        "write-char",
        "Write a character to port",
        Arity::Variadic(1),
        |args| {
            let ch = args[0].as_char()?;
            if args.len() > 1 {
                write_to_port(&args[1], &ch.to_string())?;
            } else {
                print!("{ch}");
            }
            Ok(Value::Void)
        },
    );

    // R7RS exact/inexact aliases (§6.2.6)
    vm.register_fn(
        "exact",
        "Convert to exact",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Float(f) => Ok(Value::Int(*f as i64)),
            Value::Int(_) => Ok(args[0].clone()),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    vm.register_fn(
        "inexact",
        "Convert to inexact",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Int(n) => Ok(Value::Float(*n as f64)),
            Value::Float(_) => Ok(args[0].clone()),
            _ => Err(LispError::type_error("number", format!("{}", args[0]))),
        },
    );

    // R7RS §6.13.2 File I/O
    vm.register_fn(
        "open-input-file",
        "Open file for reading",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            let file = std::fs::File::open(path)
                .map_err(|e| LispError::user(format!("open-input-file: {e}"), vec![]))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileInput {
                    reader: Box::new(std::io::BufReader::new(file)),
                    name: path.to_string(),
                },
            ))))
        },
    );

    vm.register_fn(
        "open-output-file",
        "Open file for writing",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            let file = std::fs::File::create(path)
                .map_err(|e| LispError::user(format!("open-output-file: {e}"), vec![]))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileOutput {
                    writer: Box::new(std::io::BufWriter::new(file)),
                    name: path.to_string(),
                },
            ))))
        },
    );

    // R7RS §6.14 System interface
    vm.register_fn(
        "get-environment-variable",
        "Get environment variable value",
        Arity::Fixed(1),
        |args| {
            let name = args[0].as_str()?;
            match std::env::var(name) {
                Ok(val) => Ok(Value::String(Rc::from(val.as_str()))),
                Err(_) => Ok(Value::Bool(false)),
            }
        },
    );

    vm.register_fn(
        "get-environment-variables",
        "Get all environment variables as alist",
        Arity::Fixed(0),
        |_args| {
            let pairs: Vec<Value> = std::env::vars()
                .map(|(k, v)| {
                    Value::cons(
                        Value::String(Rc::from(k.as_str())),
                        Value::String(Rc::from(v.as_str())),
                    )
                })
                .collect();
            Ok(Value::list(pairs))
        },
    );

    vm.register_fn(
        "command-line",
        "Return command-line arguments",
        Arity::Fixed(0),
        |_args| {
            let args: Vec<Value> = std::env::args()
                .map(|a| Value::String(Rc::from(a.as_str())))
                .collect();
            Ok(Value::list(args))
        },
    );

    // R7RS §6.14 current-second (TAI seconds since epoch)
    vm.register_fn(
        "current-second",
        "Current time in seconds since epoch",
        Arity::Fixed(0),
        |_args| {
            use std::time::SystemTime;
            let secs = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            Ok(Value::Float(secs))
        },
    );

    vm.register_fn(
        "current-jiffy",
        "Current time in jiffies (nanoseconds)",
        Arity::Fixed(0),
        |_args| {
            use std::time::SystemTime;
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            Ok(Value::Int(nanos as i64))
        },
    );

    vm.register_fn(
        "jiffies-per-second",
        "Number of jiffies per second",
        Arity::Fixed(0),
        |_args| Ok(Value::Int(1_000_000_000)),
    );

    // R7RS write-simple (no shared structure notation)
    vm.register_fn(
        "write-simple",
        "Write value without shared structure notation",
        Arity::Variadic(1),
        |args| {
            let text = format!("{}", args[0]);
            if args.len() > 1 {
                write_to_port(&args[1], &text)?;
            } else {
                print!("{text}");
            }
            Ok(Value::Void)
        },
    );

    // write-shared (same as write for now — no shared structure support)
    vm.register_fn(
        "write-shared",
        "Write value with shared structure notation",
        Arity::Variadic(1),
        |args| {
            let text = format!("{}", args[0]);
            if args.len() > 1 {
                write_to_port(&args[1], &text)?;
            } else {
                print!("{text}");
            }
            Ok(Value::Void)
        },
    );

    // R7RS §6.13.2 read-string — read k characters from port
    vm.register_fn(
        "read-string",
        "Read k characters from port",
        Arity::Variadic(1),
        |args| {
            let k = args[0].as_int()? as usize;
            let port_val = if args.len() > 1 {
                args[1].clone()
            } else {
                return Err(LispError::user(
                    "read-string: port argument required",
                    vec![],
                ));
            };
            if let Value::Port(port_rc) = &port_val {
                let mut port = port_rc.borrow_mut();
                let mut result = String::with_capacity(k);
                for _ in 0..k {
                    match &mut *port {
                        Port::StringInput { data, pos } => {
                            if let Some(ch) = data[*pos..].chars().next() {
                                result.push(ch);
                                *pos += ch.len_utf8();
                            } else {
                                break;
                            }
                        }
                        Port::FileInput { reader, .. } => {
                            let mut buf = [0u8; 4];
                            use std::io::Read;
                            match reader.read(&mut buf[..1]) {
                                Ok(0) => break,
                                Ok(_) => {
                                    let width = utf8_char_width(buf[0]);
                                    if width > 1 {
                                        let _ = reader.read_exact(&mut buf[1..width]);
                                    }
                                    if let Ok(s) = std::str::from_utf8(&buf[..width]) {
                                        result.push_str(s);
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        _ => return Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                if result.is_empty() {
                    Ok(Value::Eof)
                } else {
                    Ok(Value::String(Rc::from(result.as_str())))
                }
            } else {
                Err(LispError::type_error("port", format!("{port_val}")))
            }
        },
    );

    // R7RS §6.14 exit / emergency-exit
    vm.register_fn("exit", "Exit the program", Arity::Variadic(0), |args| {
        let code = if args.is_empty() {
            0
        } else {
            match &args[0] {
                Value::Bool(true) => 0,
                Value::Bool(false) => 1,
                Value::Int(n) => *n as i32,
                _ => 0,
            }
        };
        Err(LispError::user(format!("exit: {code}"), vec![]))
    });

    vm.register_fn(
        "emergency-exit",
        "Emergency exit (immediate)",
        Arity::Variadic(0),
        |args| {
            let code = if args.is_empty() {
                0
            } else {
                match &args[0] {
                    Value::Bool(true) => 0,
                    Value::Bool(false) => 1,
                    Value::Int(n) => *n as i32,
                    _ => 0,
                }
            };
            std::process::exit(code);
        },
    );

    // -- (scheme file) library --

    vm.register_fn(
        "file-exists?",
        "Does file exist?",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            Ok(Value::Bool(std::path::Path::new(path).exists()))
        },
    );

    vm.register_fn("delete-file", "Delete a file", Arity::Fixed(1), |args| {
        let path = args[0].as_str()?;
        std::fs::remove_file(path)
            .map_err(|e| LispError::user(format!("delete-file: {e}"), vec![]))?;
        Ok(Value::Void)
    });

    vm.register_fn(
        "open-binary-input-file",
        "Open binary input file",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            let file = std::fs::File::open(path)
                .map_err(|e| LispError::user(format!("open-binary-input-file: {e}"), vec![]))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileInput {
                    reader: Box::new(file),
                    name: path.to_string(),
                },
            ))))
        },
    );

    vm.register_fn(
        "open-binary-output-file",
        "Open binary output file",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            let file = std::fs::File::create(path)
                .map_err(|e| LispError::user(format!("open-binary-output-file: {e}"), vec![]))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileOutput {
                    writer: Box::new(file),
                    name: path.to_string(),
                },
            ))))
        },
    );

    // -- sleep/timing --

    vm.register_fn(
        "sleep-ms",
        "Sleep for N milliseconds (blocking)",
        Arity::Fixed(1),
        |args| {
            let ms = args[0].as_int()?.max(0) as u64;
            std::thread::sleep(std::time::Duration::from_millis(ms));
            Ok(Value::Bool(true))
        },
    );

    // NOTE: with-input-from-file / with-output-to-file require dynamic parameters
    // to redirect current-input/output-port. Deferred to Phase 13e.

    // (scheme load) — `load` reads a file and returns its contents as a string.
    // Full R7RS load (eval in interaction environment) requires VM access;
    // use `include` for compile-time file inclusion instead.
    vm.register_fn(
        "load",
        "Read file contents (use include for compile-time inclusion)",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            let content = std::fs::read_to_string(path)
                .map_err(|e| LispError::user(format!("load: {e}"), vec![]))?;
            Ok(Value::String(Rc::from(content.as_str())))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(code: &str) -> Value {
        let mut vm = Vm::new();
        crate::stdlib::register_stdlib(&mut vm);
        vm.eval(code).unwrap()
    }

    #[test]
    fn test_string_ports() {
        assert_eq!(
            eval("(let ((p (open-input-string \"hello\"))) (read-char p))"),
            Value::Char('h')
        );
    }

    #[test]
    fn test_output_string_port() {
        assert_eq!(
            eval(
                "(let ((p (open-output-string))) (write-string \"hello\" p) (get-output-string p))"
            ),
            Value::String(Rc::from("hello"))
        );
    }

    #[test]
    fn test_peek_char() {
        assert_eq!(
            eval("(let ((p (open-input-string \"ab\"))) (peek-char p) (read-char p))"),
            Value::Char('a')
        );
    }

    #[test]
    fn test_eof() {
        assert_eq!(
            eval("(let ((p (open-input-string \"\"))) (eof-object? (read-char p)))"),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_port_predicates() {
        assert_eq!(
            eval("(input-port? (open-input-string \"x\"))"),
            Value::Bool(true)
        );
        assert_eq!(
            eval("(output-port? (open-output-string))"),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_format() {
        assert_eq!(
            eval("(format \"~a is ~a\" 42 \"cool\")"),
            Value::String(Rc::from("42 is cool"))
        );
    }
}
