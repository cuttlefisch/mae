//! A component's data file and the shape it must have (ADR-119 H2, P-160).
//!
//! A data file is JSON. TOML is the manifest's format because people write
//! it; data files are as often written by a release script, whose natural
//! output is JSON. And TOML has its own date and time types, so a TOML data
//! file would carry dates two ways (a native date and a string), each needing
//! its own check; in JSON a date is a string, and [`iso_date`] is its one
//! check.
//!
//! The shape is deliberately small, declared in the manifest beside the file:
//! one record, or a list of records, each field one of [`FieldType`],
//! optional when its type ends in `?`. A record may hold no field the shape
//! does not declare. No schema library: this is all a component needs, and
//! the whole of it fits on a screen.

use std::fmt;

use serde_json::{Map, Value};

/// The most bytes a data file may hold (ADR-119 D21: one KB's components
/// must not cost the daemon unbounded memory at startup).
pub const MAX_DATA_BYTES: usize = 256 * 1024;

/// The most records a list may hold. With [`super::FUEL_LIMIT`]: the
/// fixture's site-changes page lists this many within the limit, a test
/// shows.
pub const MAX_DATA_ITEMS: usize = 500;

/// The type of one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Any JSON string.
    String,
    /// A string [`iso_date`] reads.
    Date,
    /// A string that can be a node's address ([`crate::address::valid_id`]).
    Id,
    /// A JSON integer that fits an `i64`.
    Integer,
    /// `true` or `false`.
    Boolean,
}

impl FieldType {
    fn parse(s: &str) -> Option<FieldType> {
        Some(match s {
            "string" => FieldType::String,
            "date" => FieldType::Date,
            "id" => FieldType::Id,
            "integer" => FieldType::Integer,
            "boolean" => FieldType::Boolean,
            _ => return None,
        })
    }
}

/// A data file's declared shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    /// A list of records (`true`) or one record.
    pub list: bool,
    /// Each field: name, type, whether it may be absent or null.
    pub fields: Vec<(String, FieldType, bool)>,
}

/// Why a declared field could not be read: its name or its type.
pub fn parse_field(name: &str, ty: &str) -> Result<(String, FieldType, bool), String> {
    if !(name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    {
        return Err(format!(
            "field `{name}` is not a plain name ([a-z_][a-z0-9_]*, at most 64), which a template could not read as `item.{name}`"
        ));
    }
    let (base, optional) = match ty.strip_suffix('?') {
        Some(b) => (b, true),
        None => (ty, false),
    };
    let t = FieldType::parse(base).ok_or_else(|| {
        format!("field `{name}` has type `{ty}`, not one of string, date, id, integer, boolean (each optionally followed by `?`)")
    })?;
    Ok((name.to_string(), t, optional))
}

impl Shape {
    /// The data in `text`, checked against this shape, with every absent
    /// optional field set to null so a template can test it. Every problem
    /// is returned, each naming where it is.
    pub fn validate(&self, text: &str) -> Result<Value, Vec<String>> {
        if text.len() > MAX_DATA_BYTES {
            return Err(vec![format!(
                "is {} bytes; a data file may hold at most {MAX_DATA_BYTES}",
                text.len()
            )]);
        }
        let mut v: Value =
            serde_json::from_str(text).map_err(|e| vec![format!("is not JSON: {e}")])?;
        let mut errors = Vec::new();
        match (&mut v, self.list) {
            (Value::Array(items), true) => {
                if items.len() > MAX_DATA_ITEMS {
                    errors.push(format!(
                        "holds {} records; a list may hold at most {MAX_DATA_ITEMS}",
                        items.len()
                    ));
                }
                for (i, item) in items.iter_mut().enumerate() {
                    self.record(item, &format!("[{i}]"), &mut errors);
                }
            }
            (v, false) => self.record(v, "", &mut errors),
            (_, true) => errors.push("is not a JSON list, as its shape declares".into()),
        }
        if errors.is_empty() {
            Ok(v)
        } else {
            Err(errors)
        }
    }

    fn record(&self, v: &mut Value, at: &str, errors: &mut Vec<String>) {
        let Value::Object(map) = v else {
            errors.push(if at.is_empty() {
                "is not a JSON object, as its shape declares".into()
            } else {
                format!("{at}: is not a JSON object")
            });
            return;
        };
        for key in map.keys() {
            if !self.fields.iter().any(|(n, _, _)| n == key) {
                errors.push(format!("{at}.{key}: is not a declared field"));
            }
        }
        for (name, ty, optional) in &self.fields {
            field(map, name, *ty, *optional, &format!("{at}.{name}"), errors);
        }
    }
}

fn field(
    map: &mut Map<String, Value>,
    name: &str,
    ty: FieldType,
    optional: bool,
    at: &str,
    errors: &mut Vec<String>,
) {
    let v = map.entry(name.to_string()).or_insert(Value::Null);
    let ok = match (ty, &*v) {
        (_, Value::Null) => {
            if !optional {
                errors.push(format!("{at}: is required"));
            }
            return;
        }
        (FieldType::String, Value::String(_)) => true,
        (FieldType::Date, Value::String(s)) => iso_date(s).is_some(),
        (FieldType::Id, Value::String(s)) => crate::address::valid_id(s),
        (FieldType::Integer, Value::Number(n)) => n.is_i64(),
        (FieldType::Boolean, Value::Bool(_)) => true,
        _ => false,
    };
    if !ok {
        errors.push(format!("{at}: is not {}", describe(ty)));
    }
}

fn describe(ty: FieldType) -> &'static str {
    match ty {
        FieldType::String => "a string",
        FieldType::Date => "an ISO date (YYYY-MM-DD, a real day, year 0001 to 9999)",
        FieldType::Id => "a node ID that can be an address",
        FieldType::Integer => "an integer",
        FieldType::Boolean => "true or false",
    }
}

/// A calendar date read from `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct IsoDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl fmt::Display for IsoDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// The date `s` names, when it is exactly `YYYY-MM-DD` with ASCII digits and
/// a real day of the proleptic Gregorian calendar, year 0001 to 9999 (HTML's
/// valid date string, as `<time datetime>` needs it). Anything else (a time,
/// an Org timestamp, February 30th, year 0000) is `None`; it never panics.
/// The result displays as `s`.
pub fn iso_date(s: &str) -> Option<IsoDate> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u16> {
        b[r].iter().try_fold(0u16, |acc, d| {
            d.is_ascii_digit().then(|| acc * 10 + u16::from(d - b'0'))
        })
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (year >= 1 && (1..=days).contains(&day)).then_some(IsoDate {
        year,
        month: month as u8,
        day: day as u8,
    })
}
