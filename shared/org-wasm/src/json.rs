//! A strict JSON reader (RFC 8259) for requests: no dependency, so the
//! module needs nothing but mae-org. Tested against serde_json
//! (`tests/json.rs`). Nesting deeper than [`MAX_DEPTH`] is refused, so a
//! hostile request cannot exhaust the stack. A lone UTF-16 surrogate in a
//! `\u` escape reads as U+FFFD, as `TextEncoder` encodes one.

/// A JSON value. Objects keep their keys in order; a repeated key keeps its
/// last value, as `JSON.parse` does.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// The deepest nesting a request may have.
pub const MAX_DEPTH: usize = 64;

/// Parses `s`, which must hold exactly one JSON value.
pub fn parse(s: &str) -> Result<Json, String> {
    let mut r = Reader {
        b: s.as_bytes(),
        i: 0,
    };
    let v = r.value(0)?;
    r.ws();
    if r.i != r.b.len() {
        return Err(r.err("trailing characters"));
    }
    Ok(v)
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn err(&self, what: &str) -> String {
        format!("invalid JSON at byte {}: {what}", self.i)
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        let ok = self.b[self.i..].starts_with(lit.as_bytes());
        if ok {
            self.i += lit.len();
        }
        ok
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        self.ws();
        match self.b.get(self.i) {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') if self.eat("true") => Ok(Json::Bool(true)),
            Some(b'f') if self.eat("false") => Ok(Json::Bool(false)),
            Some(b'n') if self.eat("null") => Ok(Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.err("expected a value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.i += 1;
        let mut out: Vec<(String, Json)> = Vec::new();
        // Where each key is, so a repeat is one lookup (a resolver can hold
        // many thousands of keys).
        let mut at: mae_org::seeded::HashMap<String, usize> = Default::default();
        self.ws();
        if self.eat("}") {
            return Ok(Json::Obj(out));
        }
        loop {
            self.ws();
            if self.b.get(self.i) != Some(&b'"') {
                return Err(self.err("expected a key"));
            }
            let k = self.string()?;
            self.ws();
            if !self.eat(":") {
                return Err(self.err("expected ':'"));
            }
            let v = self.value(depth + 1)?;
            match at.get(&k) {
                Some(&i) => out[i].1 = v,
                None => {
                    at.insert(k.clone(), out.len());
                    out.push((k, v));
                }
            }
            self.ws();
            if self.eat("}") {
                return Ok(Json::Obj(out));
            }
            if !self.eat(",") {
                return Err(self.err("expected ',' or '}'"));
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.eat("]") {
            return Ok(Json::Arr(out));
        }
        loop {
            out.push(self.value(depth + 1)?);
            self.ws();
            if self.eat("]") {
                return Ok(Json::Arr(out));
            }
            if !self.eat(",") {
                return Err(self.err("expected ',' or ']'"));
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        self.eat("-");
        let digits = |r: &mut Self| {
            let s = r.i;
            while r.i < r.b.len() && r.b[r.i].is_ascii_digit() {
                r.i += 1;
            }
            r.i - s
        };
        let int_start = self.i;
        let n = digits(self);
        if n == 0 || (n > 1 && self.b[int_start] == b'0') {
            return Err(self.err("malformed number"));
        }
        if self.eat(".") && digits(self) == 0 {
            return Err(self.err("malformed fraction"));
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if !self.eat("+") {
                self.eat("-");
            }
            if digits(self) == 0 {
                return Err(self.err("malformed exponent"));
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("number"))?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| self.err("malformed number"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self
            .b
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("short \\u"))?;
        let s = std::str::from_utf8(h).map_err(|_| self.err("bad \\u"))?;
        let v = u32::from_str_radix(s, 16).map_err(|_| self.err("bad \\u"))?;
        if !h.iter().all(u8::is_ascii_hexdigit) {
            return Err(self.err("bad \\u"));
        }
        self.i += 4;
        Ok(v)
    }

    fn escape(&mut self, out: &mut String) -> Result<(), String> {
        let c = *self.b.get(self.i).ok_or_else(|| self.err("short escape"))?;
        self.i += 1;
        let ch = match c {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode(out),
            _ => return Err(self.err("unknown escape")),
        };
        out.push(ch);
        Ok(())
    }

    fn unicode(&mut self, out: &mut String) -> Result<(), String> {
        let hi = self.hex4()?;
        let c = if (0xD800..0xDC00).contains(&hi) && self.b[self.i..].starts_with(b"\\u") {
            let save = self.i;
            self.i += 2;
            let lo = self.hex4()?;
            if (0xDC00..0xE000).contains(&lo) {
                char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
            } else {
                self.i = save;
                None
            }
        } else {
            char::from_u32(hi)
        };
        out.push(c.unwrap_or('\u{fffd}'));
        Ok(())
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.b.len() && !matches!(self.b[self.i], b'"' | b'\\' | 0..=0x1f) {
                self.i += 1;
            }
            // The input is a &str and the run stops only at ASCII bytes, so
            // the run is whole characters.
            out.push_str(
                std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("utf-8"))?,
            );
            match self.b.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    self.escape(&mut out)?;
                }
                Some(_) => return Err(self.err("control character in a string")),
                None => return Err(self.err("unterminated string")),
            }
        }
    }
}

impl Json {
    /// The value of `key` in an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}
