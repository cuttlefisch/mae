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
//! the process-level stdin/stdout/stderr. Port redirection via
//! `with-input-from-file` / `with-output-to-file` is implemented in the
//! Scheme bootstrap (base.rs) using `dynamic-wind` + internal port setters.
//!
//! ### Binary I/O
//! `read-u8`, `peek-u8`, `write-u8`, `read-bytevector`, `write-bytevector`
//! operate on bytevectors. `binary-port?` returns `#f` for text ports (all
//! file ports are opened in text mode by default).
//!
//! ### String ports
//! `open-input-string` and `open-output-string` / `get-output-string` provide
//! in-memory I/O. These are the most commonly used port types in extension code.

use std::cell::RefCell;
use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::reader::Reader;
use crate::value::{display_value, Port, Value};
use crate::vm::Vm;

/// Check if file descriptor has data available for reading (non-blocking).
/// Uses POSIX `poll(2)` with timeout=0 for an instantaneous check.
#[cfg(unix)]
fn fd_ready(fd: libc::c_int) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pfd, 1, 0) };
    result > 0 && (pfd.revents & libc::POLLIN) != 0
}

/// Fallback for non-Unix: always report ready (conservative).
#[cfg(not(unix))]
fn fd_ready(_fd: i32) -> bool {
    true
}

/// Read one UTF-8 character from stdin.
/// Reads bytes one at a time to handle multi-byte characters correctly.
fn read_char_from_stdin() -> Result<Value, LispError> {
    use std::io::Read;
    let mut buf = [0u8; 4];
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    match handle.read(&mut buf[..1]) {
        Ok(0) => Ok(Value::Eof),
        Ok(_) => {
            let needed = utf8_char_width(buf[0]);
            if needed > 1 {
                handle
                    .read_exact(&mut buf[1..needed])
                    .map_err(|e| LispError::user(format!("read-char: stdin: {e}"), vec![]))?;
            }
            let s = std::str::from_utf8(&buf[..needed]).unwrap_or("\u{FFFD}");
            Ok(Value::Char(s.chars().next().unwrap_or('\u{FFFD}')))
        }
        Err(e) => Err(LispError::user(format!("read-char: stdin: {e}"), vec![])),
    }
}

/// Read one line from stdin.
fn read_line_from_stdin() -> Result<Value, LispError> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut line = String::new();
    match handle.read_line(&mut line) {
        Ok(0) => Ok(Value::Eof),
        Ok(_) => {
            // Strip trailing newline (and \r\n on Windows)
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Ok(Value::String(Rc::from(line.as_str())))
        }
        Err(e) => Err(LispError::user(format!("read-line: stdin: {e}"), vec![])),
    }
}

/// Create a LispError with a proper R7RS file-error tagged object.
fn file_error(message: String, path: &str) -> LispError {
    let err_obj = Value::Vector(Rc::new(RefCell::new(vec![
        Value::symbol("error-object"),
        Value::string(message.clone()),
        Value::string("file-error"),
        Value::list(vec![Value::string(path)]),
    ])));
    let mut err = LispError::user(message, vec![path.to_string()]);
    err.error_value = Some(Box::new(err_obj));
    err
}

// Note: read-error objects are synthesized by the VM's handle_exception()
// from ErrorKind::Read. No explicit read_error() helper needed here.

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
/// Write raw bytes to a port (for write-bytevector, write-u8).
fn write_bytes_to_port(port_val: &Value, bytes: &[u8]) -> Result<(), LispError> {
    match port_val {
        Value::Port(port_cell) => {
            let mut port = port_cell.borrow_mut();
            match &mut *port {
                Port::Closed(_) => Err(LispError::user("write: port is closed", vec![])),
                Port::BytevectorOutput { buf } => {
                    buf.extend_from_slice(bytes);
                    Ok(())
                }
                Port::StringOutput { buf } => {
                    // Best-effort: interpret bytes as Latin-1 for string ports
                    for &b in bytes {
                        buf.push(b as char);
                    }
                    Ok(())
                }
                Port::Stdout => {
                    use std::io::Write;
                    std::io::stdout()
                        .write_all(bytes)
                        .map_err(|e| LispError::internal(format!("write error: {e}")))
                }
                Port::Stderr => {
                    use std::io::Write;
                    std::io::stderr()
                        .write_all(bytes)
                        .map_err(|e| LispError::internal(format!("write error: {e}")))
                }
                Port::FileOutput { writer, .. } => {
                    use std::io::Write;
                    writer
                        .write_all(bytes)
                        .map_err(|e| LispError::internal(format!("write error: {e}")))
                }
                _ => Err(LispError::type_error("output-port", "input-port")),
            }
        }
        _ => Err(LispError::type_error("port", format!("{port_val}"))),
    }
}

fn write_to_port(port_val: &Value, text: &str) -> Result<(), LispError> {
    match port_val {
        Value::Port(port_cell) => {
            let mut port = port_cell.borrow_mut();
            match &mut *port {
                Port::Closed(_) => Err(LispError::user("write: port is closed", vec![])),
                Port::StringOutput { buf } => {
                    buf.push_str(text);
                    Ok(())
                }
                Port::BytevectorOutput { buf } => {
                    buf.extend_from_slice(text.as_bytes());
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
    // Create shared mutable cells for current ports — allows dynamic redirection
    // by with-input-from-file / with-output-to-file via dynamic-wind.
    let stdin_port = Value::Port(Rc::new(RefCell::new(Port::Stdin { peeked: None })));
    let stdout_port = Value::Port(Rc::new(RefCell::new(Port::Stdout)));
    let stderr_port = Value::Port(Rc::new(RefCell::new(Port::Stderr)));

    let current_in: Rc<RefCell<Value>> = Rc::new(RefCell::new(stdin_port));
    let current_out: Rc<RefCell<Value>> = Rc::new(RefCell::new(stdout_port));
    let current_err: Rc<RefCell<Value>> = Rc::new(RefCell::new(stderr_port));

    let co = current_out.clone();
    vm.register_fn(
        "display",
        "Display value (human-readable, no quotes on strings)",
        Arity::Variadic(1),
        move |args| {
            let text = display_value(&args[0]);
            if args.len() > 1 {
                write_to_port(&args[1], &text)?;
            } else {
                write_to_port(&co.borrow(), &text)?;
            }
            Ok(Value::Void)
        },
    );

    let co = current_out.clone();
    vm.register_fn(
        "write",
        "Write value (machine-readable, with quotes)",
        Arity::Variadic(1),
        move |args| {
            let text = format!("{}", args[0]);
            if args.len() > 1 {
                write_to_port(&args[1], &text)?;
            } else {
                write_to_port(&co.borrow(), &text)?;
            }
            Ok(Value::Void)
        },
    );

    // R7RS §6.13.2 read — read one S-expression from port
    let ci = current_in.clone();
    vm.register_fn(
        "read",
        "Read one S-expression from port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed(_) => Err(LispError::user("read: port is closed", vec![])),
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
                            name,
                            binary: false,
                            text_buf,
                            text_pos,
                            reader,
                        } => {
                            // Lazily buffer all text content
                            if text_buf.is_none() {
                                use std::io::Read;
                                let mut contents = String::new();
                                reader.read_to_string(&mut contents).map_err(|e| {
                                    LispError::internal(format!("read: error reading {name}: {e}"))
                                })?;
                                *text_buf = Some(contents);
                            }
                            let buf = text_buf.as_ref().unwrap();
                            if *text_pos >= buf.len() {
                                return Ok(Value::Eof);
                            }
                            let remaining = &buf[*text_pos..];
                            let mut r = Reader::new(remaining, name.as_str());
                            match r.read() {
                                Ok(Some(val)) => {
                                    *text_pos += r.position();
                                    Ok(val)
                                }
                                Ok(None) => Ok(Value::Eof),
                                Err(e) => Err(e),
                            }
                        }
                        Port::FileInput { binary: true, .. } => Err(LispError::user(
                            "read: cannot read from binary port",
                            vec![],
                        )),
                        Port::Stdin { peeked } => {
                            // Read a line from stdin, then parse as S-expression
                            let prefix = peeked.take().map(|ch| ch.to_string());
                            match read_line_from_stdin()? {
                                Value::Eof => {
                                    // Try parsing any peeked char as datum
                                    if let Some(p) = prefix {
                                        let mut reader = Reader::new(&p, "<stdin>");
                                        match reader.read() {
                                            Ok(Some(val)) => Ok(val),
                                            Ok(None) => Ok(Value::Eof),
                                            Err(e) => Err(e),
                                        }
                                    } else {
                                        Ok(Value::Eof)
                                    }
                                }
                                Value::String(s) => {
                                    let input = if let Some(p) = prefix {
                                        format!("{p}{s}")
                                    } else {
                                        s.to_string()
                                    };
                                    let mut reader = Reader::new(&input, "<stdin>");
                                    match reader.read() {
                                        Ok(Some(val)) => Ok(val),
                                        Ok(None) => Ok(Value::Eof),
                                        Err(e) => Err(e),
                                    }
                                }
                                _ => Ok(Value::Eof),
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "output-port")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    let co = current_out.clone();
    vm.register_fn(
        "newline",
        "Print newline",
        Arity::Variadic(0),
        move |args| {
            if !args.is_empty() {
                write_to_port(&args[0], "\n")?;
            } else {
                write_to_port(&co.borrow(), "\n")?;
            }
            Ok(Value::Void)
        },
    );

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

    // read-char from port (or current-input-port)
    let ci = current_in.clone();
    vm.register_fn(
        "read-char",
        "Read a character from port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed(_) => {
                            Err(LispError::user("read-char: port is closed", vec![]))
                        }
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = data[*pos..].chars().next().unwrap();
                                *pos += ch.len_utf8();
                                Ok(Value::Char(ch))
                            }
                        }
                        Port::FileInput {
                            binary: false,
                            text_buf,
                            text_pos,
                            reader,
                            ..
                        } => {
                            if text_buf.is_none() {
                                use std::io::Read;
                                let mut contents = String::new();
                                let _ = reader.read_to_string(&mut contents);
                                *text_buf = Some(contents);
                            }
                            let buf = text_buf.as_ref().unwrap();
                            if *text_pos >= buf.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = buf[*text_pos..].chars().next().unwrap();
                                *text_pos += ch.len_utf8();
                                Ok(Value::Char(ch))
                            }
                        }
                        Port::FileInput {
                            binary: true,
                            reader,
                            ..
                        } => {
                            use std::io::Read;
                            let mut buf = [0u8; 4];
                            match reader.read(&mut buf[..1]) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(_) => {
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
                        Port::Stdin { peeked } => {
                            // Return peeked char if available, otherwise read from stdin
                            if let Some(ch) = peeked.take() {
                                Ok(Value::Char(ch))
                            } else {
                                read_char_from_stdin()
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    // peek-char from port (or current-input-port)
    let ci = current_in.clone();
    vm.register_fn(
        "peek-char",
        "Peek at next character from port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed(_) => {
                            Err(LispError::user("peek-char: port is closed", vec![]))
                        }
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = data[*pos..].chars().next().unwrap();
                                Ok(Value::Char(ch))
                            }
                        }
                        Port::FileInput {
                            binary: false,
                            text_buf,
                            text_pos,
                            reader,
                            ..
                        } => {
                            if text_buf.is_none() {
                                use std::io::Read;
                                let mut contents = String::new();
                                let _ = reader.read_to_string(&mut contents);
                                *text_buf = Some(contents);
                            }
                            let buf = text_buf.as_ref().unwrap();
                            if *text_pos >= buf.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = buf[*text_pos..].chars().next().unwrap();
                                Ok(Value::Char(ch))
                            }
                        }
                        Port::Stdin { peeked } => {
                            // Peek: read char from stdin, store it for next read-char
                            if let Some(ch) = *peeked {
                                Ok(Value::Char(ch))
                            } else {
                                match read_char_from_stdin()? {
                                    Value::Eof => Ok(Value::Eof),
                                    Value::Char(ch) => {
                                        *peeked = Some(ch);
                                        Ok(Value::Char(ch))
                                    }
                                    other => Ok(other),
                                }
                            }
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    // write-string to port — R7RS §6.13.2: (write-string string [port [start [end]]])
    let co = current_out.clone();
    vm.register_fn(
        "write-string",
        "Write string (or substring) to port",
        Arity::Variadic(1),
        move |args| {
            let s = args[0].as_str()?;
            let port = if args.len() > 1 {
                args[1].clone()
            } else {
                co.borrow().clone()
            };
            if args.len() > 2 {
                // start/end range
                let chars: Vec<char> = s.chars().collect();
                let start = args[2].as_int()? as usize;
                let end = if args.len() > 3 {
                    args[3].as_int()? as usize
                } else {
                    chars.len()
                };
                let sub: String = chars[start..end].iter().collect();
                write_to_port(&port, &sub)?;
            } else {
                write_to_port(&port, s)?;
            }
            Ok(Value::Void)
        },
    );

    // Port predicates — R7RS §6.13.1: predicates return #t even on closed ports
    vm.register_fn(
        "input-port?",
        "Is input port? (returns #t even when closed)",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => Ok(Value::Bool(p.borrow().is_input())),
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "output-port?",
        "Is output port? (returns #t even when closed)",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => Ok(Value::Bool(p.borrow().is_output())),
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
        |args| match &args[0] {
            Value::Port(p) => Ok(Value::Bool(!p.borrow().is_binary())),
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "binary-port?",
        "Is binary port?",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Port(p) => Ok(Value::Bool(p.borrow().is_binary())),
            _ => Ok(Value::Bool(false)),
        },
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
            let kind = p.borrow().kind();
            *p.borrow_mut() = Port::Closed(kind);
        }
        Ok(Value::Void)
    });

    vm.register_fn(
        "close-input-port",
        "Close input port",
        Arity::Fixed(1),
        |args| {
            if let Value::Port(p) = &args[0] {
                let kind = p.borrow().kind();
                *p.borrow_mut() = Port::Closed(kind);
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
                let kind = p.borrow().kind();
                *p.borrow_mut() = Port::Closed(kind);
            }
            Ok(Value::Void)
        },
    );

    let co = current_out.clone();
    vm.register_fn(
        "flush-output-port",
        "Flush output port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                co.borrow().clone()
            } else {
                args[0].clone()
            };
            if let Value::Port(p) = &port_val {
                let mut port = p.borrow_mut();
                match &mut *port {
                    Port::FileOutput { writer, .. } => {
                        use std::io::Write;
                        writer
                            .flush()
                            .map_err(|e| LispError::user(format!("flush: {e}"), vec![]))?;
                    }
                    Port::Stdout => {
                        use std::io::Write;
                        std::io::stdout()
                            .flush()
                            .map_err(|e| LispError::user(format!("flush: {e}"), vec![]))?;
                    }
                    Port::Stderr => {
                        use std::io::Write;
                        std::io::stderr()
                            .flush()
                            .map_err(|e| LispError::user(format!("flush: {e}"), vec![]))?;
                    }
                    _ => {} // String ports don't need flushing
                }
            }
            Ok(Value::Void)
        },
    );

    // read-line from input port
    let ci = current_in.clone();
    vm.register_fn(
        "read-line",
        "Read a line from input port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::Closed(_) => {
                            Err(LispError::user("read-line: port is closed", vec![]))
                        }
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
                        Port::FileInput {
                            binary: false,
                            text_buf,
                            text_pos,
                            reader,
                            ..
                        } => {
                            if text_buf.is_none() {
                                use std::io::Read;
                                let mut contents = String::new();
                                let _ = reader.read_to_string(&mut contents);
                                *text_buf = Some(contents);
                            }
                            let buf = text_buf.as_ref().unwrap();
                            if *text_pos >= buf.len() {
                                return Ok(Value::Eof);
                            }
                            let remaining = &buf[*text_pos..];
                            if let Some(nl) = remaining.find('\n') {
                                let line = &remaining[..nl];
                                *text_pos += nl + 1;
                                // Strip trailing \r
                                let line = line.strip_suffix('\r').unwrap_or(line);
                                Ok(Value::String(Rc::from(line)))
                            } else {
                                let line = remaining;
                                *text_pos = buf.len();
                                Ok(Value::String(Rc::from(line)))
                            }
                        }
                        Port::FileInput {
                            binary: true,
                            reader,
                            ..
                        } => {
                            use std::io::BufRead;
                            let mut line = String::new();
                            let reader: &mut dyn std::io::Read = &mut **reader;
                            let mut buf_reader = std::io::BufReader::new(reader);
                            match buf_reader.read_line(&mut line) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(_) => {
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
                        Port::Stdin { peeked } => {
                            // If there's a peeked char, prepend it to the line
                            let prefix = peeked.take().map(|ch| ch.to_string());
                            match read_line_from_stdin()? {
                                Value::Eof => {
                                    if let Some(p) = prefix {
                                        Ok(Value::String(Rc::from(p.as_str())))
                                    } else {
                                        Ok(Value::Eof)
                                    }
                                }
                                Value::String(s) => {
                                    if let Some(p) = prefix {
                                        let combined = format!("{p}{s}");
                                        Ok(Value::String(Rc::from(combined.as_str())))
                                    } else {
                                        Ok(Value::String(s))
                                    }
                                }
                                other => Ok(other),
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
            ]))
        },
    );

    // R7RS §6.13.1 Standard ports
    // Current ports use shared cells so with-input-from-file/with-output-to-file
    // can temporarily redirect them.
    // current-input/output/error-port use the shared cells created at top
    let ci = current_in.clone();
    vm.register_fn(
        "current-input-port",
        "Current default input port",
        Arity::Fixed(0),
        move |_args| Ok(ci.borrow().clone()),
    );

    let co = current_out.clone();
    vm.register_fn(
        "current-output-port",
        "Current default output port",
        Arity::Fixed(0),
        move |_args| Ok(co.borrow().clone()),
    );

    let ce = current_err;
    vm.register_fn(
        "current-error-port",
        "Current default error port",
        Arity::Fixed(0),
        move |_args| Ok(ce.borrow().clone()),
    );

    // Internal getters/setters for with-input-from-file / with-output-to-file
    let ci = current_in.clone();
    vm.register_fn(
        "%current-input-port",
        "Get current input port (internal)",
        Arity::Fixed(0),
        move |_args| Ok(ci.borrow().clone()),
    );

    let co = current_out.clone();
    vm.register_fn(
        "%current-output-port",
        "Get current output port (internal)",
        Arity::Fixed(0),
        move |_args| Ok(co.borrow().clone()),
    );

    let ci = current_in.clone();
    vm.register_fn(
        "%set-current-input-port!",
        "Set current input port (internal)",
        Arity::Fixed(1),
        move |args| {
            *ci.borrow_mut() = args[0].clone();
            Ok(Value::Void)
        },
    );

    let co = current_out.clone();
    vm.register_fn(
        "%set-current-output-port!",
        "Set current output port (internal)",
        Arity::Fixed(1),
        move |args| {
            *co.borrow_mut() = args[0].clone();
            Ok(Value::Void)
        },
    );

    // R7RS §6.13.3 Binary I/O — bytevector ports
    vm.register_fn(
        "open-input-bytevector",
        "Create input port from bytevector",
        Arity::Fixed(1),
        |args| match &args[0] {
            Value::Bytevector(bv) => {
                let data = bv.borrow().clone();
                Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                    Port::BytevectorInput { data, pos: 0 },
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
                Port::BytevectorOutput { buf: Vec::new() },
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
                    Port::BytevectorOutput { buf } => Ok(Value::bytevector(buf.clone())),
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
    let ci = current_in.clone();
    vm.register_fn(
        "read-u8",
        "Read a byte from port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
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
                        Port::BytevectorInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let byte = data[*pos];
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
                        Port::Stdin { .. } => {
                            use std::io::Read;
                            let mut buf = [0u8; 1];
                            match std::io::stdin().lock().read(&mut buf) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(_) => Ok(Value::Int(buf[0] as i64)),
                                Err(e) => Err(LispError::user(format!("read-u8: {e}"), vec![])),
                            }
                        }
                        Port::Closed(_) => Err(LispError::user("read-u8: port is closed", vec![])),
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", args[0]))),
            }
        },
    );

    let ci = current_in.clone();
    vm.register_fn(
        "peek-u8",
        "Peek at next byte from port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
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
                        Port::BytevectorInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                Ok(Value::Int(data[*pos] as i64))
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
        move |args| {
            let byte = args[0].as_int()? as u8;
            if args.len() > 1 {
                write_bytes_to_port(&args[1], &[byte])?;
            } else {
                use std::io::Write;
                std::io::stdout()
                    .write_all(&[byte])
                    .map_err(|e| LispError::internal(format!("write error: {e}")))?;
            }
            Ok(Value::Void)
        },
    );

    // R7RS §6.13.3 read-bytevector, read-bytevector!, write-bytevector
    let ci = current_in.clone();
    vm.register_fn(
        "read-bytevector",
        "Read k bytes from port",
        Arity::Variadic(1),
        move |args| {
            let k = args[0].as_int()? as usize;
            let port_val = if args.len() > 1 {
                args[1].clone()
            } else {
                ci.borrow().clone()
            };
            match &port_val {
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
                        Port::BytevectorInput { data, pos } => {
                            if *pos >= data.len() {
                                return Ok(Value::Eof);
                            }
                            let end = (*pos + k).min(data.len());
                            let bytes = data[*pos..end].to_vec();
                            *pos = end;
                            Ok(Value::bytevector(bytes))
                        }
                        Port::FileInput { reader, .. } => {
                            use std::io::Read;
                            let mut buf = vec![0u8; k];
                            match reader.read(&mut buf) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(n) => {
                                    buf.truncate(n);
                                    Ok(Value::bytevector(buf))
                                }
                                Err(e) => {
                                    Err(LispError::user(format!("read-bytevector: {e}"), vec![]))
                                }
                            }
                        }
                        Port::Closed(_) => {
                            Err(LispError::user("read-bytevector: port is closed", vec![]))
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", port_val))),
            }
        },
    );

    // R7RS §6.13.3 read-bytevector! — read into existing bytevector
    let ci = current_in.clone();
    vm.register_fn(
        "read-bytevector!",
        "Read bytes into bytevector, return count or eof",
        Arity::Variadic(2),
        move |args| {
            let bv = match &args[0] {
                Value::Bytevector(bv) => bv.clone(),
                _ => return Err(LispError::type_error("bytevector", format!("{}", args[0]))),
            };
            let port_val = if args.len() > 1 {
                args[1].clone()
            } else {
                ci.borrow().clone()
            };
            let start = if args.len() > 2 {
                args[2].as_int()? as usize
            } else {
                0
            };
            let end = if args.len() > 3 {
                args[3].as_int()? as usize
            } else {
                bv.borrow().len()
            };
            match &port_val {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                return Ok(Value::Eof);
                            }
                            let src = data.as_bytes();
                            let mut bv_mut = bv.borrow_mut();
                            let mut count = 0;
                            for i in start..end {
                                if *pos >= src.len() {
                                    break;
                                }
                                bv_mut[i] = src[*pos];
                                *pos += 1;
                                count += 1;
                            }
                            if count == 0 {
                                Ok(Value::Eof)
                            } else {
                                Ok(Value::Int(count))
                            }
                        }
                        Port::BytevectorInput { data, pos } => {
                            if *pos >= data.len() {
                                return Ok(Value::Eof);
                            }
                            let mut bv_mut = bv.borrow_mut();
                            let mut count = 0;
                            for i in start..end {
                                if *pos >= data.len() {
                                    break;
                                }
                                bv_mut[i] = data[*pos];
                                *pos += 1;
                                count += 1;
                            }
                            if count == 0 {
                                Ok(Value::Eof)
                            } else {
                                Ok(Value::Int(count))
                            }
                        }
                        Port::FileInput { reader, .. } => {
                            use std::io::Read;
                            let mut bv_mut = bv.borrow_mut();
                            match reader.read(&mut bv_mut[start..end]) {
                                Ok(0) => Ok(Value::Eof),
                                Ok(n) => Ok(Value::Int(n as i64)),
                                Err(e) => {
                                    Err(LispError::user(format!("read-bytevector!: {e}"), vec![]))
                                }
                            }
                        }
                        Port::Closed(_) => {
                            Err(LispError::user("read-bytevector!: port is closed", vec![]))
                        }
                        _ => Err(LispError::type_error("input-port", "other port type")),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", port_val))),
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
                if args.len() > 1 {
                    write_bytes_to_port(&args[1], slice)?;
                } else {
                    use std::io::Write;
                    std::io::stdout()
                        .write_all(slice)
                        .map_err(|e| LispError::internal(format!("write error: {e}")))?;
                }
                Ok(Value::Void)
            }
            _ => Err(LispError::type_error("bytevector", format!("{}", args[0]))),
        },
    );

    // R7RS §6.13.2 char-ready? — returns #t if read-char would not block.
    // For string ports: check if data remains. For file/stdin: #t (conservative).
    let ci = current_in.clone();
    vm.register_fn(
        "char-ready?",
        "Returns #t if a character is ready on the input port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        Port::StringInput { data, pos } => Ok(Value::Bool(*pos < data.len())),
                        Port::Closed(_) => {
                            Err(LispError::user("char-ready?: port is closed", vec![]))
                        }
                        Port::FileInput {
                            binary: false,
                            text_buf: Some(buf),
                            text_pos,
                            ..
                        } => Ok(Value::Bool(*text_pos < buf.len())),
                        Port::Stdin { peeked, .. } => {
                            // If there's a peeked char, definitely ready.
                            // Otherwise, use poll(2) to check stdin fd 0.
                            if peeked.is_some() {
                                Ok(Value::Bool(true))
                            } else {
                                Ok(Value::Bool(fd_ready(0)))
                            }
                        }
                        // Unbuffered file ports: regular files always return
                        // POLLIN from poll(2) — they never block. This is
                        // correct per POSIX, not a conservative approximation.
                        // At EOF, R7RS §6.13.2 requires #t as well.
                        _ => Ok(Value::Bool(true)),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", port_val))),
            }
        },
    );

    // R7RS §6.13.3 u8-ready? — same semantics for binary ports.
    let ci = current_in.clone();
    vm.register_fn(
        "u8-ready?",
        "Returns #t if a byte is ready on the input port",
        Arity::Variadic(0),
        move |args| {
            let port_val = if args.is_empty() {
                ci.borrow().clone()
            } else {
                args[0].clone()
            };
            match &port_val {
                Value::Port(p) => {
                    let port = p.borrow();
                    match &*port {
                        Port::StringInput { data, pos } => Ok(Value::Bool(*pos < data.len())),
                        Port::BytevectorInput { data, pos } => Ok(Value::Bool(*pos < data.len())),
                        Port::Stdin { peeked, .. } => {
                            // If there's a peeked char, a byte is definitely available.
                            // Otherwise, use poll(2) to check stdin fd 0.
                            if peeked.is_some() {
                                Ok(Value::Bool(true))
                            } else {
                                Ok(Value::Bool(fd_ready(0)))
                            }
                        }
                        Port::Closed(_) => {
                            Err(LispError::user("u8-ready?: port is closed", vec![]))
                        }
                        // Regular file ports: disk I/O never blocks in the
                        // poll(2) sense. POSIX guarantees POLLIN for regular files.
                        _ => Ok(Value::Bool(true)),
                    }
                }
                _ => Err(LispError::type_error("port", format!("{}", port_val))),
            }
        },
    );

    // R7RS §6.13.2 write-char with port support
    let co = current_out.clone();
    vm.register_fn(
        "write-char",
        "Write a character to port",
        Arity::Variadic(1),
        move |args| {
            let ch = args[0].as_char()?;
            if args.len() > 1 {
                write_to_port(&args[1], &ch.to_string())?;
            } else {
                write_to_port(&co.borrow(), &ch.to_string())?;
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
                .map_err(|e| file_error(format!("open-input-file: {e}"), path))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileInput {
                    reader: Box::new(std::io::BufReader::new(file)),
                    name: path.to_string(),
                    binary: false,
                    text_buf: None,
                    text_pos: 0,
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
                .map_err(|e| file_error(format!("open-output-file: {e}"), path))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileOutput {
                    writer: Box::new(std::io::BufWriter::new(file)),
                    name: path.to_string(),
                    binary: false,
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
    let ci = current_in.clone();
    vm.register_fn(
        "read-string",
        "Read k characters from port",
        Arity::Variadic(1),
        move |args| {
            let k = args[0].as_int()? as usize;
            let port_val = if args.len() > 1 {
                args[1].clone()
            } else {
                ci.borrow().clone()
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
                        Port::FileInput {
                            binary: false,
                            text_buf,
                            text_pos,
                            reader,
                            ..
                        } => {
                            if text_buf.is_none() {
                                use std::io::Read;
                                let mut contents = String::new();
                                let _ = reader.read_to_string(&mut contents);
                                *text_buf = Some(contents);
                            }
                            let buf = text_buf.as_ref().unwrap();
                            if let Some(ch) = buf[*text_pos..].chars().next() {
                                result.push(ch);
                                *text_pos += ch.len_utf8();
                            } else {
                                break;
                            }
                        }
                        Port::FileInput {
                            binary: true,
                            reader,
                            ..
                        } => {
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
                        Port::Stdin { peeked } => {
                            // Use peeked char first, then read from stdin
                            if let Some(ch) = peeked.take() {
                                result.push(ch);
                            } else {
                                match read_char_from_stdin() {
                                    Ok(Value::Char(ch)) => result.push(ch),
                                    _ => break,
                                }
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
        std::fs::remove_file(path).map_err(|e| file_error(format!("delete-file: {e}"), path))?;
        Ok(Value::Void)
    });

    vm.register_fn(
        "open-binary-input-file",
        "Open binary input file",
        Arity::Fixed(1),
        |args| {
            let path = args[0].as_str()?;
            let file = std::fs::File::open(path)
                .map_err(|e| file_error(format!("open-binary-input-file: {e}"), path))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileInput {
                    reader: Box::new(file),
                    name: path.to_string(),
                    binary: true,
                    text_buf: None,
                    text_pos: 0,
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
                .map_err(|e| file_error(format!("open-binary-output-file: {e}"), path))?;
            Ok(Value::Port(Rc::new(std::cell::RefCell::new(
                Port::FileOutput {
                    writer: Box::new(file),
                    name: path.to_string(),
                    binary: true,
                },
            ))))
        },
    );

    // -- sleep/timing (yield-based) --

    vm.register_fn(
        "sleep-ms",
        "Sleep for N milliseconds (yields to event loop)",
        Arity::Fixed(1),
        |args| {
            let ms = args[0].as_int()?.max(0) as u64;
            Err(LispError::yield_sleep(std::time::Duration::from_millis(ms)))
        },
    );

    vm.register_fn(
        "wait-for-file",
        "Wait until file exists (yields to event loop)",
        Arity::Fixed(2),
        |args| {
            let path = args[0]
                .as_str()
                .map_err(|_| LispError::type_error("string", args[0].type_name()))?;
            let timeout_ms = args[1].as_int()?.max(0) as u64;
            Err(LispError::yield_wait_for_file(
                std::path::PathBuf::from(path),
                std::time::Duration::from_millis(timeout_ms),
            ))
        },
    );

    vm.register_fn(
        "current-milliseconds",
        "Return the current time in milliseconds since the Unix epoch",
        Arity::Fixed(0),
        |_args| {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            Ok(Value::Int(ms))
        },
    );

    // -- flush! (yield-based pending op flush) --
    vm.register_fn(
        "flush!",
        "Flush pending ops and refresh editor state mid-eval (yields to host)",
        Arity::Fixed(0),
        |_args| Err(LispError::yield_flush()),
    );

    // -- yield-tick (yield one event loop iteration) --
    vm.register_fn(
        "yield-tick",
        "Yield to the event loop for one iteration, letting hooks and side effects drain. Returns #t.",
        Arity::Fixed(0),
        |_args| Err(LispError::yield_tick()),
    );

    // -- await-hook (suspend until named hook fires or timeout) --
    vm.register_fn(
        "await-hook",
        "Suspend until the named hook fires or timeout (ms) expires. Returns #t if hook fired, #f on timeout.",
        Arity::Fixed(2),
        |args| {
            let name = args[0]
                .as_str()
                .map_err(|_| LispError::type_error("string", args[0].type_name()))?;
            let timeout_ms = args[1].as_int()?.max(0) as u64;
            Err(LispError::yield_await_hook(
                name.to_string(),
                std::time::Duration::from_millis(timeout_ms),
            ))
        },
    );

    // with-input-from-file / with-output-to-file are implemented in Scheme
    // (stdlib/base.rs bootstrap) using dynamic-wind + %set-current-input-port!.

    // (scheme load) — `load` is a compiler special form (Op::Load) that reads
    // and evaluates a file in the interaction environment. See compiler.rs.
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
