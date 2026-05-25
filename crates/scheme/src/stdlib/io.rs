//! R7RS §6.13: I/O and display primitives.

use std::rc::Rc;

use crate::lisp_error::{Arity, LispError};
use crate::value::{display_value, Port, Value};
use crate::vm::Vm;

/// Write a string to a port value.
fn write_to_port(port_val: &Value, text: &str) -> Result<(), LispError> {
    match port_val {
        Value::Port(port_cell) => {
            let mut port = port_cell.borrow_mut();
            match &mut *port {
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

    vm.register_fn("newline", "Print newline", Arity::Variadic(0), |args| {
        if !args.is_empty() {
            write_to_port(&args[0], "\n")?;
        } else {
            println!();
        }
        Ok(Value::Void)
    });

    vm.register_fn("write-char", "Write a character", Arity::Fixed(1), |args| {
        print!("{}", args[0].as_char()?);
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
                // Read from stdin — not supported in embedded context
                return Ok(Value::Eof);
            }
            match &args[0] {
                Value::Port(p) => {
                    let mut port = p.borrow_mut();
                    match &mut *port {
                        crate::value::Port::StringInput { data, pos } => {
                            if *pos >= data.len() {
                                Ok(Value::Eof)
                            } else {
                                let ch = data[*pos..].chars().next().unwrap();
                                *pos += ch.len_utf8();
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
                        crate::value::Port::StringInput { data, pos } => {
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
                match &args[1] {
                    Value::Port(p) => {
                        let mut port = p.borrow_mut();
                        match &mut *port {
                            crate::value::Port::StringOutput { buf: data } => {
                                data.push_str(s);
                                Ok(Value::Void)
                            }
                            _ => Err(LispError::type_error("output-port", "other port type")),
                        }
                    }
                    _ => Err(LispError::type_error("port", format!("{}", args[1]))),
                }
            } else {
                print!("{s}");
                Ok(Value::Void)
            }
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
                Ok(Value::Bool(matches!(*port, Port::StringInput { .. })))
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
                Ok(Value::Bool(matches!(*port, Port::StringOutput { .. })))
            }
            _ => Ok(Value::Bool(false)),
        },
    );

    vm.register_fn(
        "close-port",
        "Close a port",
        Arity::Fixed(1),
        |_args| Ok(Value::Void), // No-op for string ports
    );

    vm.register_fn(
        "close-input-port",
        "Close input port",
        Arity::Fixed(1),
        |_args| Ok(Value::Void),
    );

    vm.register_fn(
        "close-output-port",
        "Close output port",
        Arity::Fixed(1),
        |_args| Ok(Value::Void),
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
                Value::symbol("ratios"),
                Value::symbol("exact-complex"),
            ]))
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
