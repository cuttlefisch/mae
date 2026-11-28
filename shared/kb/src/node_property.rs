//! What a node property may be, so that writing one through any surface (the
//! `kb-set-node-property!` Scheme primitive, the `kb_set_property` MCP tool)
//! always survives the node's org round trip: [`crate::export::node_to_org`]
//! renders `properties` as a `:PROPERTIES:` drawer, and both org readers
//! (`crate::org::parse_org`, the node buffer's; and mae-org's ingest,
//! `crate::org_ingest::stored_properties`) read it back.
//!
//! The rules come from those readers, not from taste:
//! - a key is the run between two colons with no blank in it, and the legacy
//!   reader splits the line at its second colon, so a key holds neither;
//! - the readers lower-case ASCII only while the export upper-cases fully, so
//!   a key outside ASCII would come back spelled differently;
//! - `ID`, `KIND` and `ALIASES` are node fields, not properties; `END` closes
//!   the drawer and `PROPERTIES` opens one; a trailing `+` is org's
//!   accumulate syntax;
//! - the readers trim the value and drop an empty one, so a value is one line
//!   with no blank at either end, and removal is spelled as removal;
//! - `role` carries the molecular-note role, so its values are that set.

use std::fmt;

/// The molecular-note roles `role` accepts.
pub const ROLES: &[&str] = &["source", "atom", "molecule", "hub"];

/// Keys that are not properties: node fields, or drawer syntax.
const RESERVED: &[&str] = &["id", "kind", "aliases", "end", "properties"];

/// Why a property write is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyRefusal {
    EmptyKey,
    /// A character outside printable ASCII, a blank, or `:`.
    KeyCharacter(char),
    /// `ID`, `KIND`, `ALIASES`, `END` or `PROPERTIES`.
    ReservedKey(String),
    /// A trailing `+` (org's accumulate syntax).
    KeyAccumulates,
    /// An empty value: remove the property instead.
    EmptyValue,
    /// A line break in the value.
    MultilineValue,
    /// A blank at the start or end of the value, which the readers trim.
    ValueBlankEdge,
    /// A `role` that is not one of [`ROLES`].
    UnknownRole(String),
}

impl fmt::Display for PropertyRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyKey => write!(f, "the property key is empty"),
            Self::KeyCharacter(c) => write!(
                f,
                "the property key may hold printable ASCII only, without blanks or ':' (found {c:?})"
            ),
            Self::ReservedKey(k) => write!(
                f,
                "'{k}' is not a property (a node field, or drawer syntax)"
            ),
            Self::KeyAccumulates => write!(f, "a property key may not end in '+'"),
            Self::EmptyValue => write!(f, "the value is empty: remove the property instead"),
            Self::MultilineValue => write!(f, "the value must be one line"),
            Self::ValueBlankEdge => write!(f, "the value may not start or end with a blank"),
            Self::UnknownRole(r) => write!(
                f,
                "Invalid role '{r}': expected {}",
                ROLES.join("|")
            ),
        }
    }
}

/// `key` as the store keeps it (ASCII lower-case), if a property may be
/// written under it with `value` (`None`: removed).
pub fn validate(key: &str, value: Option<&str>) -> Result<String, PropertyRefusal> {
    if key.is_empty() {
        return Err(PropertyRefusal::EmptyKey);
    }
    if let Some(c) = key.chars().find(|c| !c.is_ascii_graphic() || *c == ':') {
        return Err(PropertyRefusal::KeyCharacter(c));
    }
    let key = key.to_ascii_lowercase();
    if RESERVED.contains(&key.as_str()) {
        return Err(PropertyRefusal::ReservedKey(key));
    }
    if key.ends_with('+') {
        return Err(PropertyRefusal::KeyAccumulates);
    }
    let Some(value) = value else {
        return Ok(key);
    };
    if value.is_empty() {
        return Err(PropertyRefusal::EmptyValue);
    }
    if value.contains(['\n', '\r']) {
        return Err(PropertyRefusal::MultilineValue);
    }
    if value.trim() != value {
        return Err(PropertyRefusal::ValueBlankEdge);
    }
    if key == "role" && !ROLES.contains(&value) {
        return Err(PropertyRefusal::UnknownRole(value.to_string()));
    }
    Ok(key)
}

#[cfg(test)]
#[path = "node_property_tests.rs"]
mod tests;
