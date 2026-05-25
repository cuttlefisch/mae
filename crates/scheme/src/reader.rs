//! mae-scheme reader: recursive descent S-expression parser.
//!
//! Parses R7RS §7.1 lexical structure into Value AST.
//! Supports: atoms, lists, vectors, bytevectors, quoting,
//! quasiquote, datum comments, block comments, datum labels.
//!
//! @stability: unstable (Phase 13)
//! @since: 0.12.0

use std::collections::HashMap;

use crate::lisp_error::{LispError, SourceLocation};
use crate::value::{intern, Value};

/// Reader state: tracks position in source for error reporting.
pub struct Reader<'a> {
    input: &'a str,
    pos: usize,
    line: u32,
    column: u32,
    file: String,
    /// Datum label definitions: #N= ...
    datum_labels: HashMap<u32, Value>,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a str, file: impl Into<String>) -> Self {
        Reader {
            input,
            pos: 0,
            line: 1,
            column: 1,
            file: file.into(),
            datum_labels: HashMap::new(),
        }
    }

    /// Read a single datum from the input. Returns None at EOF.
    pub fn read(&mut self) -> Result<Option<Value>, LispError> {
        self.skip_atmosphere();
        if self.at_end() {
            return Ok(None);
        }
        Ok(Some(self.read_datum()?))
    }

    /// Read all datums from the input.
    pub fn read_all(&mut self) -> Result<Vec<Value>, LispError> {
        let mut results = Vec::new();
        while let Some(datum) = self.read()? {
            results.push(datum);
        }
        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Core reading
    // -----------------------------------------------------------------------

    fn read_datum(&mut self) -> Result<Value, LispError> {
        self.skip_atmosphere();

        if self.at_end() {
            return Err(self.error("unexpected end of input"));
        }

        let c = self.peek_char().unwrap();

        match c {
            '(' => self.read_list(),
            '#' => self.read_hash(),
            '\'' => self.read_quote("quote"),
            '`' => self.read_quote("quasiquote"),
            ',' => {
                self.advance();
                if self.peek_char() == Some('@') {
                    self.advance();
                    let datum = self.read_datum()?;
                    Ok(Value::list(vec![Value::symbol("unquote-splicing"), datum]))
                } else {
                    let datum = self.read_datum()?;
                    Ok(Value::list(vec![Value::symbol("unquote"), datum]))
                }
            }
            '"' => self.read_string(),
            ';' => unreachable!("semicolons handled by skip_atmosphere"),
            ')' => Err(self.error("unexpected ')'")),
            _ => self.read_atom(),
        }
    }

    // -----------------------------------------------------------------------
    // Lists and pairs
    // -----------------------------------------------------------------------

    fn read_list(&mut self) -> Result<Value, LispError> {
        self.expect_char('(')?;
        let mut elements = Vec::new();
        let mut dotted_cdr: Option<Value> = None;

        loop {
            self.skip_atmosphere();
            if self.at_end() {
                return Err(self.error("unterminated list"));
            }
            if self.peek_char() == Some(')') {
                self.advance();
                break;
            }

            // Check for dot (dotted pair)
            if self.peek_char() == Some('.')
                && self.is_delimiter_at(self.pos + 1)
                && !elements.is_empty()
            {
                self.advance(); // consume '.'
                dotted_cdr = Some(self.read_datum()?);
                self.skip_atmosphere();
                if self.peek_char() != Some(')') {
                    return Err(self.error("expected ')' after dotted pair cdr"));
                }
                self.advance(); // consume ')'
                break;
            }

            elements.push(self.read_datum()?);
        }

        // Build list from the end
        let mut result = dotted_cdr.unwrap_or(Value::Null);
        for elem in elements.into_iter().rev() {
            result = Value::cons(elem, result);
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Hash-prefixed forms: #t, #f, #(, #u8(, #\, #;, #|...|#, #N=, #N#
    // -----------------------------------------------------------------------

    fn read_hash(&mut self) -> Result<Value, LispError> {
        self.advance(); // consume '#'
        if self.at_end() {
            return Err(self.error("unexpected end of input after '#'"));
        }

        let c = self.peek_char().unwrap();
        match c {
            't' => {
                self.advance();
                // Allow #true
                if self.peek_char() == Some('r') {
                    self.try_consume("rue");
                }
                Ok(Value::Bool(true))
            }
            'f' => {
                self.advance();
                // Allow #false
                if self.peek_char() == Some('a') {
                    self.try_consume("alse");
                }
                Ok(Value::Bool(false))
            }
            '(' => self.read_vector(),
            'u' => {
                self.advance(); // consume 'u'
                if self.peek_char() == Some('8') {
                    self.advance(); // consume '8'
                    self.read_bytevector()
                } else {
                    Err(self.error("expected '8' after '#u'"))
                }
            }
            '\\' => {
                self.advance(); // consume '\'
                self.read_character()
            }
            ';' => {
                self.advance(); // consume ';'
                                // Datum comment: read and discard one datum
                self.read_datum()?;
                // Read the next actual datum
                self.read_datum()
            }
            '|' => {
                // Block comment (already handled in skip_atmosphere, but handle here too)
                self.advance();
                self.skip_block_comment()?;
                self.read_datum()
            }
            c if c.is_ascii_digit() => self.read_datum_label(),
            _ => Err(self.error(format!("unexpected character after '#': '{c}'"))),
        }
    }

    fn read_vector(&mut self) -> Result<Value, LispError> {
        self.expect_char('(')?;
        let mut elements = Vec::new();
        loop {
            self.skip_atmosphere();
            if self.at_end() {
                return Err(self.error("unterminated vector"));
            }
            if self.peek_char() == Some(')') {
                self.advance();
                break;
            }
            elements.push(self.read_datum()?);
        }
        Ok(Value::vector(elements))
    }

    fn read_bytevector(&mut self) -> Result<Value, LispError> {
        self.expect_char('(')?;
        let mut bytes = Vec::new();
        loop {
            self.skip_atmosphere();
            if self.at_end() {
                return Err(self.error("unterminated bytevector"));
            }
            if self.peek_char() == Some(')') {
                self.advance();
                break;
            }
            let val = self.read_datum()?;
            match val {
                Value::Int(n) if (0..=255).contains(&n) => bytes.push(n as u8),
                Value::Int(n) => {
                    return Err(self.error(format!("bytevector element out of range: {n}")))
                }
                _ => return Err(self.error("bytevector elements must be integers 0-255")),
            }
        }
        Ok(Value::bytevector(bytes))
    }

    fn read_character(&mut self) -> Result<Value, LispError> {
        if self.at_end() {
            return Err(self.error("unexpected end of input in character literal"));
        }

        let c = self.peek_char().unwrap();

        // Check for hex character #\xHEX first (before alpha check catches 'x')
        if (c == 'x' || c == 'X')
            && self.pos + c.len_utf8() < self.input.len()
            && self.input.as_bytes()[self.pos + c.len_utf8()].is_ascii_hexdigit()
        {
            self.advance(); // consume 'x'/'X'
            let hex = self.read_hex_digits()?;
            return char::from_u32(hex)
                .map(Value::Char)
                .ok_or_else(|| self.error(format!("invalid Unicode scalar value: {hex:#x}")));
        }

        // Named characters and single alphabetic characters
        if c.is_ascii_alphabetic() {
            let start = self.pos;
            while !self.at_end() && self.peek_char().is_some_and(|c| c.is_ascii_alphanumeric()) {
                self.advance();
            }
            let name = &self.input[start..self.pos];

            // Single character
            if name.len() == 1 {
                return Ok(Value::Char(name.chars().next().unwrap()));
            }

            match name {
                "space" => Ok(Value::Char(' ')),
                "newline" | "linefeed" => Ok(Value::Char('\n')),
                "return" => Ok(Value::Char('\r')),
                "tab" => Ok(Value::Char('\t')),
                "null" | "nul" => Ok(Value::Char('\0')),
                "alarm" => Ok(Value::Char('\x07')),
                "backspace" => Ok(Value::Char('\x08')),
                "escape" => Ok(Value::Char('\x1b')),
                "delete" => Ok(Value::Char('\x7f')),
                _ => Err(self.error(format!("unknown character name: {name}"))),
            }
        } else {
            // Single non-alpha character (including standalone x/X at delimiter)
            self.advance();
            Ok(Value::Char(c))
        }
    }

    fn read_datum_label(&mut self) -> Result<Value, LispError> {
        // We're after '#', next is a digit
        let mut n: u32 = 0;
        while let Some(c) = self.peek_char() {
            if let Some(d) = c.to_digit(10) {
                n = n * 10 + d;
                self.advance();
            } else {
                break;
            }
        }

        match self.peek_char() {
            Some('=') => {
                self.advance(); // consume '='
                let datum = self.read_datum()?;
                self.datum_labels.insert(n, datum.clone());
                Ok(datum)
            }
            Some('#') => {
                self.advance(); // consume '#'
                self.datum_labels
                    .get(&n)
                    .cloned()
                    .ok_or_else(|| self.error(format!("undefined datum label: #{n}#")))
            }
            _ => Err(self.error(format!("expected '=' or '#' after '#{n}'"))),
        }
    }

    // -----------------------------------------------------------------------
    // Strings
    // -----------------------------------------------------------------------

    fn read_string(&mut self) -> Result<Value, LispError> {
        self.expect_char('"')?;
        let mut result = String::new();

        loop {
            if self.at_end() {
                return Err(self.error("unterminated string"));
            }

            let c = self.peek_char().unwrap();
            match c {
                '"' => {
                    self.advance();
                    return Ok(Value::string(result));
                }
                '\\' => {
                    self.advance();
                    if self.at_end() {
                        return Err(self.error("unterminated string escape"));
                    }
                    let esc = self.peek_char().unwrap();
                    self.advance();
                    match esc {
                        'n' => result.push('\n'),
                        'r' => result.push('\r'),
                        't' => result.push('\t'),
                        '\\' => result.push('\\'),
                        '"' => result.push('"'),
                        'a' => result.push('\x07'),
                        'b' => result.push('\x08'),
                        '0' => result.push('\0'),
                        'x' => {
                            let code = self.read_hex_digits()?;
                            if self.peek_char() == Some(';') {
                                self.advance(); // consume ';'
                            }
                            let ch = char::from_u32(code).ok_or_else(|| {
                                self.error(format!("invalid Unicode scalar: {code:#x}"))
                            })?;
                            result.push(ch);
                        }
                        '\n' => {
                            // Line continuation: skip newline + leading whitespace
                            while self.peek_char().is_some_and(|c| c == ' ' || c == '\t') {
                                self.advance();
                            }
                        }
                        _ => {
                            return Err(self.error(format!("unknown string escape: \\{esc}")));
                        }
                    }
                }
                _ => {
                    result.push(c);
                    self.advance();
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Atoms: numbers, symbols, booleans
    // -----------------------------------------------------------------------

    fn read_atom(&mut self) -> Result<Value, LispError> {
        let start = self.pos;

        // Handle sign prefix
        if self.peek_char() == Some('+') || self.peek_char() == Some('-') {
            let sign_pos = self.pos;
            self.advance();
            // If followed by a delimiter or EOF, it's a symbol (+, -)
            if self.at_end() || self.is_delimiter_here() {
                self.pos = sign_pos; // reset
                return self.read_symbol();
            }
            // If followed by a digit or '.', it's a number
            let next = self.peek_char();
            if next.is_some_and(|c| c.is_ascii_digit() || c == '.') {
                self.pos = sign_pos; // reset, let read_number handle it
                return self.read_number();
            }
            // Otherwise it's a symbol like +inf.0 or a user symbol
            self.pos = sign_pos;
        }

        let c = self.peek_char().unwrap();

        if c.is_ascii_digit() {
            self.read_number()
        } else if c == '.' {
            // Could be a number like .5 or an identifier like ...
            let next_pos = self.pos + 1;
            if next_pos < self.input.len() && self.input.as_bytes()[next_pos].is_ascii_digit() {
                self.read_number()
            } else {
                self.read_symbol()
            }
        } else {
            // Check for special number identifiers
            let remaining = &self.input[start..];
            if remaining.starts_with("+inf.0")
                || remaining.starts_with("-inf.0")
                || remaining.starts_with("+nan.0")
                || remaining.starts_with("-nan.0")
            {
                self.read_special_number()
            } else {
                self.read_symbol()
            }
        }
    }

    fn read_number(&mut self) -> Result<Value, LispError> {
        let start = self.pos;

        // Optional sign
        if self.peek_char() == Some('+') || self.peek_char() == Some('-') {
            self.advance();
        }

        // Check for prefix: #b, #o, #d, #x, #e, #i
        // (These are handled before read_atom is called, but support inline)

        let mut has_dot = false;
        let mut has_e = false;
        let mut has_slash = false;

        while !self.at_end() && !self.is_delimiter_here() {
            let c = self.peek_char().unwrap();
            match c {
                '0'..='9' => self.advance(),
                '.' => {
                    has_dot = true;
                    self.advance();
                }
                'e' | 'E' => {
                    has_e = true;
                    self.advance();
                    // Optional sign after exponent
                    if self.peek_char() == Some('+') || self.peek_char() == Some('-') {
                        self.advance();
                    }
                }
                '/' => {
                    has_slash = true;
                    self.advance();
                }
                _ => break,
            }
        }

        let token = &self.input[start..self.pos];

        if has_dot || has_e {
            // Float
            token
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| self.error(format!("invalid number: {token}")))
        } else if has_slash {
            // Rational — for now parse as float
            let parts: Vec<&str> = token.split('/').collect();
            if parts.len() == 2 {
                let num: f64 = parts[0]
                    .parse()
                    .map_err(|_| self.error(format!("invalid rational: {token}")))?;
                let den: f64 = parts[1]
                    .parse()
                    .map_err(|_| self.error(format!("invalid rational: {token}")))?;
                if den == 0.0 {
                    Err(LispError::division_by_zero())
                } else {
                    Ok(Value::Float(num / den))
                }
            } else {
                Err(self.error(format!("invalid rational: {token}")))
            }
        } else {
            // Integer
            token
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| self.error(format!("invalid integer: {token}")))
        }
    }

    fn read_special_number(&mut self) -> Result<Value, LispError> {
        let start = self.pos;
        // Consume until delimiter
        while !self.at_end() && !self.is_delimiter_here() {
            self.advance();
        }
        let token = &self.input[start..self.pos];
        match token {
            "+inf.0" => Ok(Value::Float(f64::INFINITY)),
            "-inf.0" => Ok(Value::Float(f64::NEG_INFINITY)),
            "+nan.0" | "-nan.0" => Ok(Value::Float(f64::NAN)),
            _ => {
                // Fall back to symbol
                Ok(Value::Symbol(intern(token)))
            }
        }
    }

    fn read_symbol(&mut self) -> Result<Value, LispError> {
        // Check for |...| delimited identifier
        if self.peek_char() == Some('|') {
            return self.read_delimited_symbol();
        }

        let start = self.pos;
        while !self.at_end() && !self.is_delimiter_here() {
            self.advance();
        }

        if self.pos == start {
            return Err(self.error("empty symbol"));
        }

        let name = &self.input[start..self.pos];

        // R7RS: identifiers are case-insensitive in the default read
        // but mae-scheme uses case-sensitive identifiers (modern convention)
        Ok(Value::Symbol(intern(name)))
    }

    fn read_delimited_symbol(&mut self) -> Result<Value, LispError> {
        self.expect_char('|')?;
        let mut name = String::new();
        loop {
            if self.at_end() {
                return Err(self.error("unterminated delimited identifier"));
            }
            let c = self.peek_char().unwrap();
            if c == '|' {
                self.advance();
                return Ok(Value::Symbol(intern(&name)));
            }
            if c == '\\' {
                self.advance();
                if self.at_end() {
                    return Err(self.error("unterminated escape in delimited identifier"));
                }
                let esc = self.peek_char().unwrap();
                self.advance();
                name.push(esc);
            } else {
                name.push(c);
                self.advance();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Quote shorthand
    // -----------------------------------------------------------------------

    fn read_quote(&mut self, sym: &str) -> Result<Value, LispError> {
        self.advance(); // consume the quote char
        let datum = self.read_datum()?;
        Ok(Value::list(vec![Value::symbol(sym), datum]))
    }

    // -----------------------------------------------------------------------
    // Whitespace and comments
    // -----------------------------------------------------------------------

    fn skip_atmosphere(&mut self) {
        loop {
            // Skip whitespace
            while !self.at_end() && self.peek_char().is_some_and(|c| c.is_whitespace()) {
                self.advance();
            }

            if self.at_end() {
                break;
            }

            // Skip line comments
            if self.peek_char() == Some(';') {
                while !self.at_end() && self.peek_char() != Some('\n') {
                    self.advance();
                }
                continue;
            }

            // Skip block comments #| ... |#
            if self.pos + 1 < self.input.len()
                && self.input.as_bytes()[self.pos] == b'#'
                && self.input.as_bytes()[self.pos + 1] == b'|'
            {
                self.advance(); // consume '#'
                self.advance(); // consume '|'
                                // Intentionally ignore error in atmosphere skip
                let _ = self.skip_block_comment();
                continue;
            }

            break;
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), LispError> {
        let mut depth = 1u32;
        while !self.at_end() && depth > 0 {
            if self.pos + 1 < self.input.len() {
                let a = self.input.as_bytes()[self.pos];
                let b = self.input.as_bytes()[self.pos + 1];
                if a == b'#' && b == b'|' {
                    depth += 1;
                    self.advance();
                    self.advance();
                    continue;
                }
                if a == b'|' && b == b'#' {
                    depth -= 1;
                    self.advance();
                    self.advance();
                    continue;
                }
            }
            self.advance();
        }
        if depth > 0 {
            Err(self.error("unterminated block comment"))
        } else {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) {
        if let Some(c) = self.peek_char() {
            self.pos += c.len_utf8();
            if c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
        }
    }

    fn expect_char(&mut self, expected: char) -> Result<(), LispError> {
        if self.peek_char() == Some(expected) {
            self.advance();
            Ok(())
        } else {
            Err(self.error(format!(
                "expected '{}', got {:?}",
                expected,
                self.peek_char()
            )))
        }
    }

    fn try_consume(&mut self, expected: &str) {
        for ch in expected.chars() {
            if self.peek_char() == Some(ch) {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn is_delimiter_here(&self) -> bool {
        self.is_delimiter_at(self.pos)
    }

    fn is_delimiter_at(&self, pos: usize) -> bool {
        if pos >= self.input.len() {
            return true;
        }
        let c = self.input.as_bytes()[pos];
        matches!(
            c,
            b' ' | b'\t' | b'\n' | b'\r' | b'(' | b')' | b'"' | b';' | b'|'
        )
    }

    fn read_hex_digits(&mut self) -> Result<u32, LispError> {
        let start = self.pos;
        while !self.at_end() && self.peek_char().is_some_and(|c| c.is_ascii_hexdigit()) {
            self.advance();
        }
        if self.pos == start {
            return Err(self.error("expected hex digits"));
        }
        let hex = &self.input[start..self.pos];
        u32::from_str_radix(hex, 16).map_err(|_| self.error(format!("invalid hex: {hex}")))
    }

    fn error(&self, msg: impl Into<String>) -> LispError {
        LispError::read_at(
            msg,
            SourceLocation {
                file: self.file.clone(),
                line: self.line,
                column: self.column,
            },
        )
    }

    /// Current byte position in the input.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Current source location.
    pub fn location(&self) -> SourceLocation {
        SourceLocation {
            file: self.file.clone(),
            line: self.line,
            column: self.column,
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience function
// ---------------------------------------------------------------------------

/// Parse a string of Scheme code into a list of Values.
pub fn read_all(input: &str) -> Result<Vec<Value>, LispError> {
    Reader::new(input, "<string>").read_all()
}

/// Parse a single datum from a string.
pub fn read_one(input: &str) -> Result<Value, LispError> {
    let mut reader = Reader::new(input, "<string>");
    reader
        .read()?
        .ok_or_else(|| LispError::read("unexpected end of input"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn read(s: &str) -> Value {
        read_one(s).unwrap()
    }

    fn read_err(s: &str) -> String {
        read_one(s).unwrap_err().message()
    }

    // --- Atoms ---

    #[test]
    fn test_integers() {
        assert_eq!(read("42"), Value::Int(42));
        assert_eq!(read("-7"), Value::Int(-7));
        assert_eq!(read("+3"), Value::Int(3));
        assert_eq!(read("0"), Value::Int(0));
    }

    #[test]
    fn test_floats() {
        assert_eq!(read("2.75"), Value::Float(2.75));
        assert_eq!(read("-0.5"), Value::Float(-0.5));
        assert_eq!(read(".5"), Value::Float(0.5));
        assert_eq!(read("1e10"), Value::Float(1e10));
        assert_eq!(read("1.5e-3"), Value::Float(1.5e-3));
    }

    #[test]
    fn test_special_numbers() {
        assert!(read("+inf.0").as_float().unwrap().is_infinite());
        assert!(read("-inf.0").as_float().unwrap().is_infinite());
        assert!(read("+nan.0").as_float().unwrap().is_nan());
    }

    #[test]
    fn test_booleans() {
        assert_eq!(read("#t"), Value::Bool(true));
        assert_eq!(read("#f"), Value::Bool(false));
        assert_eq!(read("#true"), Value::Bool(true));
        assert_eq!(read("#false"), Value::Bool(false));
    }

    #[test]
    fn test_characters() {
        assert_eq!(read("#\\a"), Value::Char('a'));
        assert_eq!(read("#\\space"), Value::Char(' '));
        assert_eq!(read("#\\newline"), Value::Char('\n'));
        assert_eq!(read("#\\tab"), Value::Char('\t'));
        assert_eq!(read("#\\return"), Value::Char('\r'));
        assert_eq!(read("#\\null"), Value::Char('\0'));
        assert_eq!(read("#\\alarm"), Value::Char('\x07'));
        assert_eq!(read("#\\backspace"), Value::Char('\x08'));
        assert_eq!(read("#\\escape"), Value::Char('\x1b'));
        assert_eq!(read("#\\delete"), Value::Char('\x7f'));
        assert_eq!(read("#\\x41"), Value::Char('A'));
    }

    #[test]
    fn test_strings() {
        assert_eq!(read(r#""hello""#).as_str().unwrap(), "hello");
        assert_eq!(read(r#""hello\nworld""#).as_str().unwrap(), "hello\nworld");
        assert_eq!(read(r#""tab\there""#).as_str().unwrap(), "tab\there");
        assert_eq!(read(r#""esc\"quote""#).as_str().unwrap(), "esc\"quote");
        assert_eq!(read(r#""back\\slash""#).as_str().unwrap(), "back\\slash");
        assert_eq!(read(r#""\x41;""#).as_str().unwrap(), "A");
        assert_eq!(read(r#""""#).as_str().unwrap(), "");
    }

    #[test]
    fn test_symbols() {
        assert!(read("foo").is_symbol());
        assert!(read("+").is_symbol());
        assert!(read("-").is_symbol());
        assert!(read("...").is_symbol());
        assert!(read("string->number").is_symbol());
        assert!(read("list?").is_symbol());
        assert!(read("set!").is_symbol());
    }

    #[test]
    fn test_delimited_symbols() {
        let v = read("|hello world|");
        assert_eq!(v.as_symbol().unwrap().name(), "hello world");
    }

    // --- Lists ---

    #[test]
    fn test_empty_list() {
        assert_eq!(read("()"), Value::Null);
    }

    #[test]
    fn test_proper_list() {
        let list = read("(1 2 3)");
        let vec = list.to_vec().unwrap();
        assert_eq!(vec.len(), 3);
        assert_eq!(vec[0], Value::Int(1));
        assert_eq!(vec[1], Value::Int(2));
        assert_eq!(vec[2], Value::Int(3));
    }

    #[test]
    fn test_nested_list() {
        let list = read("(1 (2 3) 4)");
        let vec = list.to_vec().unwrap();
        assert_eq!(vec.len(), 3);
        assert!(vec[1].is_pair());
    }

    #[test]
    fn test_dotted_pair() {
        let pair = read("(1 . 2)");
        assert_eq!(pair.car().unwrap(), Value::Int(1));
        assert_eq!(pair.cdr().unwrap(), Value::Int(2));
    }

    #[test]
    fn test_improper_list() {
        let list = read("(1 2 . 3)");
        assert_eq!(list.car().unwrap(), Value::Int(1));
        let cdr = list.cdr().unwrap();
        assert_eq!(cdr.car().unwrap(), Value::Int(2));
        assert_eq!(cdr.cdr().unwrap(), Value::Int(3));
    }

    // --- Vectors ---

    #[test]
    fn test_vector() {
        let v = read("#(1 2 3)");
        match v {
            Value::Vector(ref vec) => {
                assert_eq!(vec.borrow().len(), 3);
            }
            _ => panic!("expected vector"),
        }
    }

    // --- Bytevectors ---

    #[test]
    fn test_bytevector() {
        let bv = read("#u8(1 2 255)");
        match bv {
            Value::Bytevector(ref v) => {
                assert_eq!(*v.borrow(), vec![1u8, 2, 255]);
            }
            _ => panic!("expected bytevector"),
        }
    }

    // --- Quoting ---

    #[test]
    fn test_quote() {
        let q = read("'foo");
        let vec = q.to_vec().unwrap();
        assert_eq!(vec.len(), 2);
        assert_eq!(vec[0].as_symbol().unwrap().name(), "quote");
        assert_eq!(vec[1].as_symbol().unwrap().name(), "foo");
    }

    #[test]
    fn test_quasiquote() {
        let q = read("`(a ,b ,@c)");
        let vec = q.to_vec().unwrap();
        assert_eq!(vec[0].as_symbol().unwrap().name(), "quasiquote");
    }

    #[test]
    fn test_unquote() {
        let q = read(",x");
        let vec = q.to_vec().unwrap();
        assert_eq!(vec[0].as_symbol().unwrap().name(), "unquote");
    }

    #[test]
    fn test_unquote_splicing() {
        let q = read(",@x");
        let vec = q.to_vec().unwrap();
        assert_eq!(vec[0].as_symbol().unwrap().name(), "unquote-splicing");
    }

    // --- Comments ---

    #[test]
    fn test_line_comment() {
        let vals = read_all("; comment\n42").unwrap();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0], Value::Int(42));
    }

    #[test]
    fn test_datum_comment() {
        let vals = read_all("#;(ignored) 42").unwrap();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0], Value::Int(42));
    }

    #[test]
    fn test_block_comment() {
        let vals = read_all("#| block comment |# 42").unwrap();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0], Value::Int(42));
    }

    #[test]
    fn test_nested_block_comment() {
        let vals = read_all("#| outer #| inner |# still comment |# 42").unwrap();
        assert_eq!(vals.len(), 1);
        assert_eq!(vals[0], Value::Int(42));
    }

    // --- Datum labels ---

    #[test]
    fn test_datum_label() {
        let v = read("#0=(1 2 3)");
        assert!(v.is_list());
    }

    #[test]
    fn test_datum_reference() {
        let vals = read_all("#0=42 #0#").unwrap();
        assert_eq!(vals.len(), 2);
        assert_eq!(vals[0], Value::Int(42));
        assert_eq!(vals[1], Value::Int(42));
    }

    // --- Multiple datums ---

    #[test]
    fn test_multiple_datums() {
        let vals = read_all("1 2 3").unwrap();
        assert_eq!(vals.len(), 3);
    }

    #[test]
    fn test_complex_program() {
        let code = r#"
        (define (factorial n)
          (if (<= n 1)
              1
              (* n (factorial (- n 1)))))

        (display (factorial 10))
        (newline)
        "#;
        let vals = read_all(code).unwrap();
        assert_eq!(vals.len(), 3); // define, display, newline
    }

    // --- Round-trip tests ---

    #[test]
    fn test_roundtrip_atoms() {
        let cases = vec!["42", "-7", "3.14", "#t", "#f", "foo", "()"];
        for case in cases {
            let val = read(case);
            let written = format!("{val}");
            let reread = read(&written);
            // Compare as strings since we can't use PartialEq for all types
            assert_eq!(
                format!("{val}"),
                format!("{reread}"),
                "roundtrip failed for: {case}"
            );
        }
    }

    #[test]
    fn test_roundtrip_list() {
        let val = read("(1 (2 3) 4)");
        let written = format!("{val}");
        assert_eq!(written, "(1 (2 3) 4)");
        let reread = read(&written);
        assert_eq!(format!("{reread}"), written);
    }

    #[test]
    fn test_roundtrip_string() {
        let val = read(r#""hello\nworld""#);
        let written = format!("{val}");
        assert_eq!(written, r#""hello\nworld""#);
        let reread = read(&written);
        assert_eq!(reread.as_str().unwrap(), "hello\nworld");
    }

    #[test]
    fn test_roundtrip_char() {
        let cases = vec!["#\\a", "#\\space", "#\\newline", "#\\tab"];
        for case in cases {
            let val = read(case);
            let written = format!("{val}");
            let reread = read(&written);
            assert_eq!(val.as_char().unwrap(), reread.as_char().unwrap());
        }
    }

    // --- Error cases ---

    #[test]
    fn test_unterminated_string() {
        let err = read_err(r#""unterminated"#);
        assert!(err.contains("unterminated string"));
    }

    #[test]
    fn test_unterminated_list() {
        let err = read_err("(1 2 3");
        assert!(err.contains("unterminated list"));
    }

    #[test]
    fn test_unexpected_close_paren() {
        let err = read_err(")");
        assert!(err.contains("unexpected ')'"));
    }

    #[test]
    fn test_error_has_location() {
        let err = read_one(")").unwrap_err();
        assert!(err.location.is_some());
        let loc = err.location.unwrap();
        assert_eq!(loc.line, 1);
        assert_eq!(loc.column, 1);
    }

    // --- Whitespace handling ---

    #[test]
    fn test_leading_whitespace() {
        assert_eq!(read("  42"), Value::Int(42));
    }

    #[test]
    fn test_mixed_whitespace() {
        let vals = read_all("  1\n\t2\r\n3  ").unwrap();
        assert_eq!(vals.len(), 3);
    }

    // --- Edge cases ---

    #[test]
    fn test_symbol_plus_minus() {
        assert!(read("+").is_symbol());
        assert!(read("-").is_symbol());
        assert_eq!(read("+").as_symbol().unwrap().name(), "+");
        assert_eq!(read("-").as_symbol().unwrap().name(), "-");
    }

    #[test]
    fn test_ellipsis() {
        assert!(read("...").is_symbol());
        assert_eq!(read("...").as_symbol().unwrap().name(), "...");
    }

    #[test]
    fn test_empty_input() {
        let vals = read_all("").unwrap();
        assert!(vals.is_empty());
    }

    #[test]
    fn test_only_comments() {
        let vals = read_all("; just a comment\n").unwrap();
        assert!(vals.is_empty());
    }

    #[test]
    fn test_rational() {
        let v = read("1/3");
        assert!(v.as_float().unwrap() > 0.33);
        assert!(v.as_float().unwrap() < 0.34);
    }

    #[test]
    fn test_bytevector_range_error() {
        let err = read_err("#u8(256)");
        assert!(err.contains("out of range"));
    }
}
