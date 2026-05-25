//! Comprehensive IO/Port test fixtures for mae-scheme.
//!
//! Covers R7RS §6.13 — textual and binary ports, string ports, file ports,
//! port predicates, EOF behavior, read/write operations, and edge cases.
//!
//! Run with: cargo test -p mae-scheme --test scheme_io_ports

use std::rc::Rc;

use mae_scheme::stdlib;
use mae_scheme::value::Value;
use mae_scheme::vm::Vm;

fn eval(code: &str) -> Value {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(code).unwrap()
}

/// Evaluate code and convert result to display string for comparison.
/// Needed because Value::Pair uses Rc pointer equality.
fn eval_str(code: &str) -> String {
    format!("{}", eval(code))
}

fn is_true(code: &str) {
    let result = eval(code);
    // R7RS: everything except #f is truthy
    assert!(
        result != Value::Bool(false),
        "expected truthy value, got #f: {code}"
    );
}

fn is_false(code: &str) {
    assert_eq!(eval(code), Value::Bool(false), "expected #f: {code}");
}

fn eval_err(code: &str) -> String {
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(code).unwrap_err().to_string()
}

// ============================================================
// §6.13.1 Port predicates
// ============================================================

#[test]
fn port_predicate_string_input() {
    is_true("(port? (open-input-string \"hi\"))");
    is_true("(input-port? (open-input-string \"hi\"))");
    is_false("(output-port? (open-input-string \"hi\"))");
    is_true("(textual-port? (open-input-string \"hi\"))");
    is_true("(input-port-open? (open-input-string \"hi\"))");
    is_false("(output-port-open? (open-input-string \"hi\"))");
}

#[test]
fn port_predicate_string_output() {
    is_true("(port? (open-output-string))");
    is_false("(input-port? (open-output-string))");
    is_true("(output-port? (open-output-string))");
    is_true("(textual-port? (open-output-string))");
    is_false("(input-port-open? (open-output-string))");
    is_true("(output-port-open? (open-output-string))");
}

#[test]
fn port_predicate_standard_ports() {
    is_true("(port? (current-input-port))");
    is_true("(port? (current-output-port))");
    is_true("(port? (current-error-port))");
    is_true("(input-port? (current-input-port))");
    is_true("(output-port? (current-output-port))");
    is_true("(output-port? (current-error-port))");
    is_false("(input-port? (current-output-port))");
    is_false("(output-port? (current-input-port))");
}

#[test]
fn port_predicate_non_ports() {
    is_false("(port? 42)");
    is_false("(port? \"hello\")");
    is_false("(port? #t)");
    is_false("(port? '(1 2 3))");
    is_false("(input-port? 42)");
    is_false("(output-port? \"hello\")");
}

// ============================================================
// §6.13.1 close-port / close-input-port / close-output-port
// ============================================================

#[test]
fn close_port_marks_closed() {
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (close-port p)
           (not (input-port-open? p)))",
    );
    is_true(
        "(let ((p (open-output-string)))
           (close-port p)
           (not (output-port-open? p)))",
    );
}

#[test]
fn close_input_port() {
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (close-input-port p)
           (not (input-port-open? p)))",
    );
}

#[test]
fn close_output_port() {
    is_true(
        "(let ((p (open-output-string)))
           (close-output-port p)
           (not (output-port-open? p)))",
    );
}

#[test]
fn close_port_idempotent() {
    // Closing an already-closed port should not error
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (close-port p)
           (close-port p)
           #t)",
    );
}

// ============================================================
// §6.13.2 read-char / peek-char
// ============================================================

#[test]
fn read_char_basic() {
    assert_eq!(
        eval("(let ((p (open-input-string \"abc\"))) (read-char p))"),
        Value::Char('a')
    );
}

#[test]
fn read_char_sequence() {
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"abc\")))
               (read-char p)
               (read-char p)
               (read-char p))"
        ),
        Value::Char('c')
    );
}

#[test]
fn read_char_eof_at_end() {
    is_true(
        "(let ((p (open-input-string \"x\")))
           (read-char p)
           (eof-object? (read-char p)))",
    );
}

#[test]
fn read_char_empty_string_eof() {
    is_true(
        "(let ((p (open-input-string \"\")))
           (eof-object? (read-char p)))",
    );
}

#[test]
fn read_char_unicode() {
    // Multi-byte UTF-8 characters
    assert_eq!(
        eval("(let ((p (open-input-string \"λ\"))) (read-char p))"),
        Value::Char('λ')
    );
    assert_eq!(
        eval("(let ((p (open-input-string \"日本\"))) (read-char p))"),
        Value::Char('日')
    );
    // Read second character after first
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"αβ\")))
               (read-char p)
               (read-char p))"
        ),
        Value::Char('β')
    );
}

#[test]
fn read_char_emoji() {
    assert_eq!(
        eval("(let ((p (open-input-string \"🎉\"))) (read-char p))"),
        Value::Char('🎉')
    );
}

#[test]
fn peek_char_does_not_consume() {
    assert_eq!(
        eval_str(
            "(let ((p (open-input-string \"ab\")))
               (let ((c1 (peek-char p))
                     (c2 (peek-char p))
                     (c3 (read-char p)))
                 (list c1 c2 c3)))"
        ),
        "(#\\a #\\a #\\a)"
    );
}

#[test]
fn peek_char_eof_on_empty() {
    is_true(
        "(let ((p (open-input-string \"\")))
           (eof-object? (peek-char p)))",
    );
}

#[test]
fn peek_then_read_sequence() {
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"xyz\")))
               (peek-char p)
               (read-char p)
               (peek-char p)
               (read-char p))"
        ),
        Value::Char('y')
    );
}

// ============================================================
// §6.13.2 read-line
// ============================================================

#[test]
fn read_line_basic() {
    assert_eq!(
        eval("(read-line (open-input-string \"hello\"))"),
        Value::String(Rc::from("hello"))
    );
}

#[test]
fn read_line_strips_newline() {
    assert_eq!(
        eval("(read-line (open-input-string \"hello\\nworld\"))"),
        Value::String(Rc::from("hello"))
    );
}

#[test]
fn read_line_multiple_lines() {
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"line1\\nline2\\nline3\")))
               (read-line p)
               (read-line p))"
        ),
        Value::String(Rc::from("line2"))
    );
}

#[test]
fn read_line_eof_at_end() {
    is_true(
        "(let ((p (open-input-string \"hello\")))
           (read-line p)
           (eof-object? (read-line p)))",
    );
}

#[test]
fn read_line_empty_string() {
    is_true("(eof-object? (read-line (open-input-string \"\")))");
}

#[test]
fn read_line_empty_lines() {
    // Empty lines between content
    assert_eq!(
        eval_str(
            "(let ((p (open-input-string \"\\n\\nhello\")))
               (let ((l1 (read-line p))
                     (l2 (read-line p))
                     (l3 (read-line p)))
                 (list l1 l2 l3)))"
        ),
        "(\"\" \"\" \"hello\")"
    );
}

// ============================================================
// §6.13.2 read-string
// ============================================================

#[test]
fn read_string_basic() {
    assert_eq!(
        eval("(read-string 3 (open-input-string \"hello\"))"),
        Value::String(Rc::from("hel"))
    );
}

#[test]
fn read_string_exact_length() {
    assert_eq!(
        eval("(read-string 5 (open-input-string \"hello\"))"),
        Value::String(Rc::from("hello"))
    );
}

#[test]
fn read_string_beyond_available() {
    // Should return what's available, not error
    assert_eq!(
        eval("(read-string 10 (open-input-string \"hi\"))"),
        Value::String(Rc::from("hi"))
    );
}

#[test]
fn read_string_eof_on_empty() {
    is_true("(eof-object? (read-string 5 (open-input-string \"\")))");
}

#[test]
fn read_string_zero_chars() {
    // Reading 0 characters from non-empty port
    is_true("(eof-object? (read-string 0 (open-input-string \"hello\")))");
}

#[test]
fn read_string_unicode() {
    assert_eq!(
        eval("(read-string 2 (open-input-string \"αβγ\"))"),
        Value::String(Rc::from("αβ"))
    );
}

// ============================================================
// §6.13.2 write-char / write-string / display / write / newline
// ============================================================

#[test]
fn write_char_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-char #\\H p)
               (write-char #\\i p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("Hi"))
    );
}

#[test]
fn write_string_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-string \"hello\" p)
               (write-string \" world\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("hello world"))
    );
}

#[test]
fn display_to_port_no_quotes() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("hello"))
    );
}

#[test]
fn write_to_port_with_quotes() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("\"hello\""))
    );
}

#[test]
fn newline_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display \"a\" p)
               (newline p)
               (display \"b\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("a\nb"))
    );
}

#[test]
fn display_various_types() {
    // Numbers
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display 42 p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("42"))
    );
    // Booleans
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display #t p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("#t"))
    );
    // Characters
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display #\\a p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("a"))
    );
    // Lists
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (display '(1 2 3) p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("(1 2 3)"))
    );
}

#[test]
fn write_simple_to_port() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-simple '(1 \"two\" #t) p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("(1 \"two\" #t)"))
    );
}

// ============================================================
// §6.13.2 read (S-expressions)
// ============================================================

#[test]
fn read_integer() {
    assert_eq!(eval("(read (open-input-string \"42\"))"), Value::Int(42));
}

#[test]
fn read_string() {
    assert_eq!(
        eval("(read (open-input-string \"\\\"hello\\\"\"))"),
        Value::String(Rc::from("hello"))
    );
}

#[test]
fn read_list() {
    assert_eq!(
        eval_str("(read (open-input-string \"(1 2 3)\"))"),
        "(1 2 3)"
    );
}

#[test]
fn read_symbol() {
    assert_eq!(
        eval("(read (open-input-string \"foo\"))"),
        Value::symbol("foo")
    );
}

#[test]
fn read_boolean() {
    assert_eq!(eval("(read (open-input-string \"#t\"))"), Value::Bool(true));
}

#[test]
fn read_multiple_datums() {
    // First read gets first datum
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"1 2 3\")))
               (let ((a (read p))
                     (b (read p))
                     (c (read p)))
                 (+ a b c)))"
        ),
        Value::Int(6)
    );
}

#[test]
fn read_eof_on_empty() {
    is_true("(eof-object? (read (open-input-string \"\")))");
}

#[test]
fn read_eof_after_all_consumed() {
    is_true(
        "(let ((p (open-input-string \"42\")))
           (read p)
           (eof-object? (read p)))",
    );
}

#[test]
fn read_nested_lists() {
    assert_eq!(
        eval_str("(read (open-input-string \"((a b) (c d))\"))"),
        "((a b) (c d))"
    );
}

#[test]
fn read_quoted() {
    assert_eq!(
        eval_str("(read (open-input-string \"'foo\"))"),
        "(quote foo)"
    );
}

// ============================================================
// §6.13.1 EOF object
// ============================================================

#[test]
fn eof_object_is_eof() {
    is_true("(eof-object? (eof-object))");
}

#[test]
fn eof_object_not_other_things() {
    is_false("(eof-object? 0)");
    is_false("(eof-object? #f)");
    is_false("(eof-object? '())");
    is_false("(eof-object? \"\")");
}

// ============================================================
// §6.13.2 char-ready? / u8-ready?
// ============================================================

#[test]
fn char_ready_always_true_for_string_port() {
    is_true("(char-ready? (open-input-string \"hi\"))");
    is_true("(char-ready? (open-input-string \"\"))");
}

#[test]
fn u8_ready_always_true_for_string_port() {
    is_true("(u8-ready? (open-input-string \"x\"))");
}

// ============================================================
// §6.13.3 Binary I/O — bytevector ports
// ============================================================

#[test]
fn read_u8_basic() {
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector (bytevector 10 20 30))))
               (read-u8 p))"
        ),
        Value::Int(10)
    );
}

#[test]
fn read_u8_sequence() {
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector (bytevector 10 20 30))))
               (read-u8 p)
               (read-u8 p)
               (read-u8 p))"
        ),
        Value::Int(30)
    );
}

#[test]
fn read_u8_eof() {
    is_true(
        "(let ((p (open-input-bytevector (bytevector 42))))
           (read-u8 p)
           (eof-object? (read-u8 p)))",
    );
}

#[test]
fn read_u8_empty_bytevector() {
    is_true("(eof-object? (read-u8 (open-input-bytevector (bytevector))))");
}

#[test]
fn peek_u8_basic() {
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector (bytevector 42 43))))
               (let ((a (peek-u8 p))
                     (b (read-u8 p)))
                 (= a b)))"
        ),
        Value::Bool(true)
    );
}

#[test]
fn write_u8_to_bytevector_port() {
    assert_eq!(
        eval_str(
            "(let ((p (open-output-bytevector)))
               (write-u8 65 p)
               (write-u8 66 p)
               (get-output-bytevector p))"
        ),
        "#u8(65 66)"
    );
}

#[test]
fn read_bytevector_basic() {
    assert_eq!(
        eval_str(
            "(let ((p (open-input-bytevector (bytevector 1 2 3 4 5))))
               (read-bytevector 3 p))"
        ),
        "#u8(1 2 3)"
    );
}

#[test]
fn read_bytevector_eof() {
    is_true(
        "(let ((p (open-input-bytevector (bytevector))))
           (eof-object? (read-bytevector 5 p)))",
    );
}

#[test]
fn read_bytevector_partial() {
    // Read more than available — get what's there
    assert_eq!(
        eval(
            "(let ((p (open-input-bytevector (bytevector 1 2))))
               (bytevector-length (read-bytevector 10 p)))"
        ),
        Value::Int(2)
    );
}

#[test]
fn write_bytevector_to_port() {
    assert_eq!(
        eval_str(
            "(let ((p (open-output-bytevector)))
               (write-bytevector (bytevector 10 20 30) p)
               (get-output-bytevector p))"
        ),
        "#u8(10 20 30)"
    );
}

// ============================================================
// §6.13.2 File I/O
// ============================================================

#[test]
fn file_write_and_read_back() {
    let tmp = "/tmp/mae-scheme-io-test-rw.txt";
    eval(&format!(
        "(let ((p (open-output-file \"{tmp}\")))
           (write-string \"hello world\" p)
           (close-port p))"
    ));
    assert_eq!(
        eval(&format!(
            "(let ((p (open-input-file \"{tmp}\")))
               (let ((result (read-line p)))
                 (close-port p)
                 result))"
        )),
        Value::String(Rc::from("hello world"))
    );
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn file_read_char_by_char() {
    let tmp = "/tmp/mae-scheme-io-test-chars.txt";
    std::fs::write(tmp, "ABC").unwrap();
    assert_eq!(
        eval_str(&format!(
            "(let ((p (open-input-file \"{tmp}\")))
               (let ((a (read-char p))
                     (b (read-char p))
                     (c (read-char p)))
                 (close-port p)
                 (list a b c)))"
        )),
        "(#\\A #\\B #\\C)"
    );
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn file_write_char_by_char() {
    let tmp = "/tmp/mae-scheme-io-test-wchars.txt";
    eval(&format!(
        "(let ((p (open-output-file \"{tmp}\")))
           (write-char #\\X p)
           (write-char #\\Y p)
           (write-char #\\Z p)
           (close-port p))"
    ));
    assert_eq!(std::fs::read_to_string(tmp).unwrap(), "XYZ");
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn file_port_predicates() {
    let tmp = "/tmp/mae-scheme-io-test-pred.txt";
    std::fs::write(tmp, "test").unwrap();

    is_true(&format!("(input-port? (open-input-file \"{tmp}\"))"));
    is_true(&format!("(port? (open-input-file \"{tmp}\"))"));
    is_false(&format!("(output-port? (open-input-file \"{tmp}\"))"));

    let tmp2 = "/tmp/mae-scheme-io-test-pred2.txt";
    is_true(&format!("(output-port? (open-output-file \"{tmp2}\"))"));
    is_false(&format!("(input-port? (open-output-file \"{tmp2}\"))"));

    let _ = std::fs::remove_file(tmp);
    let _ = std::fs::remove_file(tmp2);
}

#[test]
fn file_nonexistent_errors() {
    let result = eval_err("(open-input-file \"/tmp/mae-scheme-nonexistent-file-xyz.txt\")");
    assert!(
        result.contains("No such file") || result.contains("open-input-file"),
        "Expected file-not-found error, got: {result}"
    );
}

#[test]
fn file_exists_predicate() {
    let tmp = "/tmp/mae-scheme-io-test-exists.txt";
    std::fs::write(tmp, "exists").unwrap();
    is_true(&format!("(file-exists? \"{tmp}\")"));
    is_false("(file-exists? \"/tmp/mae-scheme-nonexistent-file-abc.txt\")");
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn file_delete() {
    let tmp = "/tmp/mae-scheme-io-test-delete.txt";
    std::fs::write(tmp, "delete me").unwrap();
    assert!(std::path::Path::new(tmp).exists());
    eval(&format!("(delete-file \"{tmp}\")"));
    assert!(!std::path::Path::new(tmp).exists());
}

// ============================================================
// with-output-to-file (top-level convenience)
// ============================================================

#[test]
fn with_output_to_file_basic() {
    let tmp = "/tmp/mae-scheme-io-test-with-output.txt";
    eval(&format!(
        "(with-output-to-file \"{tmp}\" (lambda () \"result\"))"
    ));
    // The file should be created (even if output isn't redirected yet)
    assert!(std::path::Path::new(tmp).exists());
    let _ = std::fs::remove_file(tmp);
}

// ============================================================
// String port: complete roundtrip scenarios
// ============================================================

#[test]
fn string_port_roundtrip_write_read() {
    // Write to output-string, read back from input-string
    assert_eq!(
        eval(
            "(let ((out (open-output-string)))
               (display 42 out)
               (display \" hello\" out)
               (let ((in (open-input-string (get-output-string out))))
                 (read in)))"
        ),
        Value::Int(42)
    );
}

#[test]
fn string_port_accumulation() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write-string \"aaa\" p)
               (write-string \"bbb\" p)
               (write-string \"ccc\" p)
               (string-length (get-output-string p)))"
        ),
        Value::Int(9)
    );
}

#[test]
fn string_port_get_output_multiple_times() {
    // get-output-string should return accumulated state each time
    assert_eq!(
        eval_str(
            "(let ((p (open-output-string)))
               (write-string \"a\" p)
               (let ((s1 (get-output-string p)))
                 (write-string \"b\" p)
                 (let ((s2 (get-output-string p)))
                   (list s1 s2))))"
        ),
        "(\"a\" \"ab\")"
    );
}

// ============================================================
// format (non-R7RS convenience)
// ============================================================

#[test]
fn format_basic() {
    assert_eq!(
        eval("(format \"~a + ~a = ~a\" 1 2 3)"),
        Value::String(Rc::from("1 + 2 = 3"))
    );
}

#[test]
fn format_tilde_percent() {
    assert_eq!(
        eval("(format \"line1~%line2\")"),
        Value::String(Rc::from("line1\nline2"))
    );
}

#[test]
fn format_tilde_tilde() {
    assert_eq!(eval("(format \"100~~\")"), Value::String(Rc::from("100~")));
}

#[test]
fn format_s_directive() {
    // ~s should use write (machine-readable) format
    assert_eq!(
        eval("(format \"~s\" \"hello\")"),
        Value::String(Rc::from("\"hello\""))
    );
}

#[test]
fn format_no_args() {
    assert_eq!(
        eval("(format \"hello world\")"),
        Value::String(Rc::from("hello world"))
    );
}

// ============================================================
// §6.14 System interface
// ============================================================

#[test]
fn current_second_returns_float() {
    is_true("(> (current-second) 0)");
    is_true("(inexact? (current-second))");
}

#[test]
fn current_jiffy_returns_integer() {
    is_true("(> (current-jiffy) 0)");
    is_true("(exact? (current-jiffy))");
}

#[test]
fn jiffies_per_second_is_billion() {
    assert_eq!(eval("(jiffies-per-second)"), Value::Int(1_000_000_000));
}

#[test]
fn command_line_returns_list() {
    is_true("(list? (command-line))");
}

#[test]
fn get_environment_variable() {
    // HOME should exist on Unix systems
    is_true("(string? (get-environment-variable \"HOME\"))");
}

#[test]
fn get_environment_variable_missing() {
    // Non-existent variable returns #f
    is_false("(get-environment-variable \"MAE_SCHEME_NONEXISTENT_VAR_XYZ\")");
}

#[test]
fn get_environment_variables_is_list() {
    is_true("(list? (get-environment-variables))");
    // Each element should be a pair
    is_true("(pair? (car (get-environment-variables)))");
}

// ============================================================
// §6.14 features
// ============================================================

#[test]
fn features_includes_r7rs() {
    is_true("(memq 'r7rs (features))");
}

#[test]
fn features_includes_mae() {
    is_true("(memq 'mae (features))");
}

// ============================================================
// Edge cases: mixing operations
// ============================================================

#[test]
fn interleaved_read_peek() {
    assert_eq!(
        eval_str(
            "(let ((p (open-input-string \"abcde\")))
               (let ((c1 (read-char p))    ; a
                     (c2 (peek-char p))     ; b (peek)
                     (c3 (read-char p))     ; b (consume)
                     (c4 (read-char p)))    ; c
                 (list c1 c2 c3 c4)))"
        ),
        "(#\\a #\\b #\\b #\\c)"
    );
}

#[test]
fn read_line_after_read_char() {
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"abc\\ndef\")))
               (read-char p)   ; consume 'a'
               (read-line p))" // should get "bc"
        ),
        Value::String(Rc::from("bc"))
    );
}

#[test]
fn read_after_partial_read_string() {
    assert_eq!(
        eval(
            "(let ((p (open-input-string \"hello world\")))
               (read-string 6 p)   ; \"hello \"
               (read-line p))" // \"world\"
        ),
        Value::String(Rc::from("world"))
    );
}

#[test]
fn write_display_write_mixed() {
    assert_eq!(
        eval(
            "(let ((p (open-output-string)))
               (write 42 p)
               (display \" \" p)
               (write \"hello\" p)
               (get-output-string p))"
        ),
        Value::String(Rc::from("42 \"hello\""))
    );
}

// ============================================================
// flush-output-port (no-op but should not error)
// ============================================================

#[test]
fn flush_output_port_no_error() {
    eval("(flush-output-port)");
    eval("(flush-output-port (open-output-string))");
}

// ============================================================
// load (reads file contents as string)
// ============================================================

#[test]
fn load_evaluates_file() {
    let tmp = "/tmp/mae-scheme-io-test-load.txt";
    std::fs::write(tmp, "(define load-test-var 42)").unwrap();
    // Top-level load evaluates file in interaction environment
    let mut vm = Vm::new();
    stdlib::register_stdlib(&mut vm);
    vm.eval(&format!("(load \"{tmp}\")")).unwrap();
    // The defined variable should be accessible
    let result = vm.eval("load-test-var").unwrap();
    assert_eq!(result, Value::Int(42));
    let _ = std::fs::remove_file(tmp);
}

#[test]
fn load_nonexistent_errors() {
    let result = eval_err("(load \"/tmp/mae-scheme-nonexistent-load.txt\")");
    assert!(
        result.contains("load") || result.contains("No such file"),
        "Expected load error, got: {result}"
    );
}

// ============================================================
// exact / inexact conversions
// ============================================================

#[test]
fn exact_from_float() {
    assert_eq!(eval("(exact 3.14)"), Value::Int(3));
}

#[test]
fn exact_from_int() {
    assert_eq!(eval("(exact 42)"), Value::Int(42));
}

#[test]
fn inexact_from_int() {
    assert_eq!(eval("(inexact 42)"), Value::Float(42.0));
}

#[test]
fn inexact_from_float() {
    assert_eq!(eval("(inexact 1.5)"), Value::Float(1.5));
}

// ============================================================
// Binary port predicates
// ============================================================

#[test]
fn bytevector_port_basic_roundtrip() {
    assert_eq!(
        eval(
            "(let ((p (open-output-bytevector)))
               (write-u8 72 p)
               (write-u8 101 p)
               (write-u8 108 p)
               (write-u8 108 p)
               (write-u8 111 p)
               (let ((bv (get-output-bytevector p)))
                 (bytevector-length bv)))"
        ),
        Value::Int(5)
    );
}

// ============================================================
// Type errors — operations on wrong port type
// ============================================================

#[test]
fn read_char_on_output_port_errors() {
    let result = eval_err("(read-char (open-output-string))");
    assert!(
        result.contains("input") || result.contains("type"),
        "Expected type error for read-char on output port, got: {result}"
    );
}

#[test]
fn write_string_on_input_port_errors() {
    let result = eval_err("(write-string \"hi\" (open-input-string \"x\"))");
    assert!(
        result.contains("output") || result.contains("type"),
        "Expected type error for write on input port, got: {result}"
    );
}

#[test]
fn display_on_input_port_errors() {
    let result = eval_err("(display 42 (open-input-string \"x\"))");
    assert!(
        result.contains("output") || result.contains("type"),
        "Expected type error for display on input port, got: {result}"
    );
}

#[test]
fn write_on_input_port_errors() {
    let result = eval_err("(write 42 (open-input-string \"x\"))");
    assert!(
        result.contains("output") || result.contains("type"),
        "Expected type error for write on input port, got: {result}"
    );
}

#[test]
fn read_on_non_port_errors() {
    let result = eval_err("(read-char 42)");
    assert!(
        result.contains("port") || result.contains("type"),
        "Expected type error for read-char on non-port, got: {result}"
    );
}

#[test]
fn get_output_string_on_input_port_errors() {
    let result = eval_err("(get-output-string (open-input-string \"x\"))");
    assert!(
        result.contains("output") || result.contains("type"),
        "Expected type error, got: {result}"
    );
}

// ============================================================
// Sleep (blocking, part of io.rs)
// ============================================================

#[test]
fn sleep_ms_basic() {
    let start = std::time::Instant::now();
    eval("(sleep-ms 50)");
    let elapsed = start.elapsed();
    assert!(elapsed.as_millis() >= 40, "sleep-ms too short: {elapsed:?}");
}

#[test]
fn sleep_ms_zero() {
    // Should not error
    eval("(sleep-ms 0)");
}
