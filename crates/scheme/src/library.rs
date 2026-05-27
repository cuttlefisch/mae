//! R7RS §5.6: Library/module system for mae-scheme.
//!
//! Implements `define-library`, `import`, and `export` as specified
//! in R7RS-small §5.2 and §5.6.
//!
//! Design follows Chibi-Scheme's approach: libraries are first-class
//! objects with a name, export list, and evaluated environment.
//! Import sets support all R7RS transformers: `only`, `except`,
//! `prefix`, `rename`.
//!
//! @stability: unstable (Phase 13d)
//! @since: 0.12.0

use std::collections::HashMap;

use crate::lisp_error::LispError;
use crate::value::Value;

/// A library name, e.g., `(scheme base)` or `(mae buffer)`.
/// Stored as a vector of name components (symbols/integers per R7RS).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LibraryName(pub Vec<String>);

impl LibraryName {
    /// Parse a library name from a Scheme value like `(scheme base)`.
    pub fn from_value(val: &Value) -> Result<Self, LispError> {
        let items = val
            .to_vec()
            .map_err(|_| LispError::syntax("library name must be a list", format!("{val}")))?;
        if items.is_empty() {
            return Err(LispError::syntax("library name must be non-empty", "()"));
        }
        let mut parts = Vec::with_capacity(items.len());
        for item in &items {
            match item {
                Value::Symbol(s) => parts.push(s.name().to_string()),
                Value::Int(n) => parts.push(n.to_string()),
                _ => {
                    return Err(LispError::syntax(
                        "library name component must be identifier or integer",
                        format!("{item}"),
                    ))
                }
            }
        }
        Ok(LibraryName(parts))
    }

    /// Convert to display string like `(scheme base)`.
    pub fn to_string_repr(&self) -> String {
        format!("({})", self.0.join(" "))
    }
}

impl std::fmt::Display for LibraryName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({})", self.0.join(" "))
    }
}

/// A resolved import set: mapping from local name → source name + library.
#[derive(Clone, Debug)]
pub struct ImportSet {
    /// The library to import from.
    pub library: LibraryName,
    /// Mapping: local_name → exported_name.
    /// If empty, import all exports (resolved at link time).
    pub bindings: ImportBindings,
}

/// How bindings are imported.
#[derive(Clone, Debug)]
pub enum ImportBindings {
    /// Import all exports from the library.
    All,
    /// Import all exports, but exclude these names.
    AllExcept(Vec<String>),
    /// Import all exports with a prefix added to each name.
    AllPrefixed(String),
    /// Import all exports, with specific renames applied (old → new).
    AllRenamed(HashMap<String, String>),
    /// Import specific bindings: local_name → exported_name.
    Explicit(HashMap<String, String>),
}

/// A library definition (the result of parsing `define-library`).
#[derive(Clone, Debug)]
pub struct LibraryDef {
    /// Library name.
    pub name: LibraryName,
    /// Export specifications: exported_name → internal_name.
    pub exports: HashMap<String, String>,
    /// Import sets (dependencies).
    pub imports: Vec<ImportSet>,
    /// Body expressions (from `begin` declarations).
    pub body: Vec<Value>,
}

/// The library registry: stores all known libraries.
#[derive(Clone, Debug, Default)]
pub struct LibraryRegistry {
    /// Registered libraries by name.
    libraries: HashMap<LibraryName, Library>,
}

/// A fully loaded library with its exported bindings.
#[derive(Clone, Debug)]
pub struct Library {
    pub name: LibraryName,
    /// Exported bindings: name → value.
    pub exports: HashMap<String, Value>,
}

impl LibraryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a library (usually after evaluating its body).
    pub fn register(&mut self, lib: Library) {
        self.libraries.insert(lib.name.clone(), lib);
    }

    /// Look up a library by name.
    pub fn get(&self, name: &LibraryName) -> Option<&Library> {
        self.libraries.get(name)
    }

    /// Check if a library is registered.
    pub fn contains(&self, name: &LibraryName) -> bool {
        self.libraries.contains_key(name)
    }

    /// List all registered library names.
    pub fn list_names(&self) -> Vec<&LibraryName> {
        self.libraries.keys().collect()
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a `(define-library <name> <decl> ...)` form.
pub fn parse_define_library(items: &[Value]) -> Result<LibraryDef, LispError> {
    // items[0] = "define-library"
    // items[1] = library name
    // items[2..] = declarations
    if items.len() < 2 {
        return Err(LispError::syntax(
            "define-library requires a name",
            format!("{}", Value::list(items.to_vec())),
        ));
    }

    let name = LibraryName::from_value(&items[1])?;
    let mut exports = HashMap::new();
    let mut imports = Vec::new();
    let mut body = Vec::new();

    for decl in &items[2..] {
        let decl_items = decl.to_vec().map_err(|_| {
            LispError::syntax("library declaration must be a list", format!("{decl}"))
        })?;
        if decl_items.is_empty() {
            continue;
        }

        match &decl_items[0] {
            Value::Symbol(s) if s.name() == "export" => {
                parse_export_specs(&decl_items[1..], &mut exports)?;
            }
            Value::Symbol(s) if s.name() == "import" => {
                for import_form in &decl_items[1..] {
                    imports.push(parse_import_set(import_form)?);
                }
            }
            Value::Symbol(s) if s.name() == "begin" => {
                body.extend_from_slice(&decl_items[1..]);
            }
            _ => {
                return Err(LispError::syntax(
                    "unknown library declaration",
                    format!("{decl}"),
                ));
            }
        }
    }

    Ok(LibraryDef {
        name,
        exports,
        imports,
        body,
    })
}

/// Parse export specs: `<id>` or `(rename <id> <id>)`.
fn parse_export_specs(
    specs: &[Value],
    exports: &mut HashMap<String, String>,
) -> Result<(), LispError> {
    for spec in specs {
        match spec {
            Value::Symbol(s) => {
                let name = s.name().to_string();
                exports.insert(name.clone(), name);
            }
            Value::Pair(_) => {
                let items = spec
                    .to_vec()
                    .map_err(|_| LispError::syntax("invalid export spec", format!("{spec}")))?;
                if items.len() == 3 {
                    if let Value::Symbol(kw) = &items[0] {
                        if kw.name() == "rename" {
                            let internal = match &items[1] {
                                Value::Symbol(s) => s.name().to_string(),
                                _ => {
                                    return Err(LispError::syntax(
                                        "export rename: expected identifier",
                                        format!("{}", items[1]),
                                    ))
                                }
                            };
                            let external = match &items[2] {
                                Value::Symbol(s) => s.name().to_string(),
                                _ => {
                                    return Err(LispError::syntax(
                                        "export rename: expected identifier",
                                        format!("{}", items[2]),
                                    ))
                                }
                            };
                            exports.insert(external, internal);
                            continue;
                        }
                    }
                }
                return Err(LispError::syntax("invalid export spec", format!("{spec}")));
            }
            _ => {
                return Err(LispError::syntax("invalid export spec", format!("{spec}")));
            }
        }
    }
    Ok(())
}

/// Parse an import set (R7RS §5.2).
///
/// Import set forms:
///   `<library-name>`
///   `(only <import-set> <id> ...)`
///   `(except <import-set> <id> ...)`
///   `(prefix <import-set> <id>)`
///   `(rename <import-set> (<id1> <id2>) ...)`
pub fn parse_import_set(form: &Value) -> Result<ImportSet, LispError> {
    let items = form
        .to_vec()
        .map_err(|_| LispError::syntax("import set must be a list", format!("{form}")))?;
    if items.is_empty() {
        return Err(LispError::syntax("empty import set", ""));
    }

    // Check if first element is a transformer keyword
    if let Value::Symbol(s) = &items[0] {
        match s.name() {
            "only" => return parse_import_only(&items),
            "except" => return parse_import_except(&items),
            "prefix" => return parse_import_prefix(&items),
            "rename" => return parse_import_rename(&items),
            _ => {}
        }
    }

    // Plain library name: (scheme base)
    let name = LibraryName::from_value(form)?;
    Ok(ImportSet {
        library: name,
        bindings: ImportBindings::All,
    })
}

/// `(only <import-set> <id> ...)`
fn parse_import_only(items: &[Value]) -> Result<ImportSet, LispError> {
    if items.len() < 2 {
        return Err(LispError::syntax(
            "only: requires import-set and identifiers",
            "",
        ));
    }
    let inner = parse_import_set(&items[1])?;
    let ids: Vec<String> = items[2..]
        .iter()
        .map(|v| match v {
            Value::Symbol(s) => Ok(s.name().to_string()),
            _ => Err(LispError::syntax(
                "only: expected identifier",
                format!("{v}"),
            )),
        })
        .collect::<Result<_, _>>()?;

    let bindings = match inner.bindings {
        ImportBindings::All
        | ImportBindings::AllExcept(_)
        | ImportBindings::AllPrefixed(_)
        | ImportBindings::AllRenamed(_) => {
            // `only` narrows to explicit names — resolved at link time
            let mut map = HashMap::new();
            for id in &ids {
                map.insert(id.clone(), id.clone());
            }
            ImportBindings::Explicit(map)
        }
        ImportBindings::Explicit(existing) => {
            let mut map = HashMap::new();
            for id in &ids {
                if let Some(source) = existing.get(id) {
                    map.insert(id.clone(), source.clone());
                } else {
                    return Err(LispError::syntax(
                        format!("only: identifier '{id}' not in import set"),
                        "",
                    ));
                }
            }
            ImportBindings::Explicit(map)
        }
    };

    Ok(ImportSet {
        library: inner.library,
        bindings,
    })
}

/// `(except <import-set> <id> ...)`
fn parse_import_except(items: &[Value]) -> Result<ImportSet, LispError> {
    if items.len() < 2 {
        return Err(LispError::syntax(
            "except: requires import-set and identifiers",
            "",
        ));
    }
    let inner = parse_import_set(&items[1])?;
    let exclude: Vec<String> = items[2..]
        .iter()
        .map(|v| match v {
            Value::Symbol(s) => Ok(s.name().to_string()),
            _ => Err(LispError::syntax(
                "except: expected identifier",
                format!("{v}"),
            )),
        })
        .collect::<Result<_, _>>()?;

    // For now, store as-is. Resolution happens at link time when we know
    // the full export list. We represent "all except X" by wrapping.
    // For simplicity, if inner is Explicit, filter now.
    let bindings = match inner.bindings {
        ImportBindings::All => ImportBindings::AllExcept(exclude),
        ImportBindings::AllExcept(mut prev) => {
            prev.extend(exclude);
            ImportBindings::AllExcept(prev)
        }
        ImportBindings::Explicit(mut existing) => {
            for id in &exclude {
                existing.remove(id);
            }
            ImportBindings::Explicit(existing)
        }
        other => other, // prefix/rename on top of except: keep as-is for now
    };

    Ok(ImportSet {
        library: inner.library,
        bindings,
    })
}

/// `(prefix <import-set> <id>)`
fn parse_import_prefix(items: &[Value]) -> Result<ImportSet, LispError> {
    if items.len() != 3 {
        return Err(LispError::syntax(
            "prefix: requires import-set and prefix identifier",
            "",
        ));
    }
    let inner = parse_import_set(&items[1])?;
    let prefix = match &items[2] {
        Value::Symbol(s) => s.name().to_string(),
        _ => {
            return Err(LispError::syntax(
                "prefix: expected identifier",
                format!("{}", items[2]),
            ))
        }
    };

    let bindings = match inner.bindings {
        ImportBindings::All => ImportBindings::AllPrefixed(prefix),
        ImportBindings::Explicit(existing) => {
            let mut map = HashMap::new();
            for (local, source) in existing {
                map.insert(format!("{prefix}{local}"), source);
            }
            ImportBindings::Explicit(map)
        }
        _ => {
            // For complex nested cases, resolve inner first then prefix
            // This is a simplification; full resolution happens at link time
            ImportBindings::AllPrefixed(prefix)
        }
    };

    Ok(ImportSet {
        library: inner.library,
        bindings,
    })
}

/// `(rename <import-set> (<id1> <id2>) ...)`
fn parse_import_rename(items: &[Value]) -> Result<ImportSet, LispError> {
    if items.len() < 2 {
        return Err(LispError::syntax("rename: requires import-set", ""));
    }
    let inner = parse_import_set(&items[1])?;
    let mut renames: HashMap<String, String> = HashMap::new();
    for pair in &items[2..] {
        let pair_items = pair
            .to_vec()
            .map_err(|_| LispError::syntax("rename: expected (old new) pair", format!("{pair}")))?;
        if pair_items.len() != 2 {
            return Err(LispError::syntax(
                "rename: expected (old new) pair",
                format!("{pair}"),
            ));
        }
        let old = match &pair_items[0] {
            Value::Symbol(s) => s.name().to_string(),
            _ => {
                return Err(LispError::syntax(
                    "rename: expected identifier",
                    format!("{}", pair_items[0]),
                ))
            }
        };
        let new = match &pair_items[1] {
            Value::Symbol(s) => s.name().to_string(),
            _ => {
                return Err(LispError::syntax(
                    "rename: expected identifier",
                    format!("{}", pair_items[1]),
                ))
            }
        };
        renames.insert(old, new);
    }

    let bindings = match inner.bindings {
        ImportBindings::All => ImportBindings::AllRenamed(renames),
        ImportBindings::Explicit(existing) => {
            let mut map = HashMap::new();
            for (local, source) in existing {
                let new_local = renames.get(&local).cloned().unwrap_or(local);
                map.insert(new_local, source);
            }
            ImportBindings::Explicit(map)
        }
        _ => ImportBindings::AllRenamed(renames),
    };

    Ok(ImportSet {
        library: inner.library,
        bindings,
    })
}

/// Resolve an import set against a library's exports.
/// Returns a mapping of local_name → Value for each imported binding.
pub fn resolve_import(
    import: &ImportSet,
    library: &Library,
) -> Result<HashMap<String, Value>, LispError> {
    let mut result = HashMap::new();

    match &import.bindings {
        ImportBindings::All => {
            for (name, value) in &library.exports {
                result.insert(name.clone(), value.clone());
            }
        }
        ImportBindings::AllExcept(excludes) => {
            for (name, value) in &library.exports {
                if !excludes.contains(name) {
                    result.insert(name.clone(), value.clone());
                }
            }
        }
        ImportBindings::AllPrefixed(prefix) => {
            for (name, value) in &library.exports {
                result.insert(format!("{prefix}{name}"), value.clone());
            }
        }
        ImportBindings::AllRenamed(renames) => {
            for (name, value) in &library.exports {
                let local = renames.get(name).cloned().unwrap_or_else(|| name.clone());
                result.insert(local, value.clone());
            }
        }
        ImportBindings::Explicit(map) => {
            for (local_name, export_name) in map {
                if let Some(value) = library.exports.get(export_name) {
                    result.insert(local_name.clone(), value.clone());
                } else {
                    return Err(LispError::syntax(
                        format!(
                            "import: '{}' not exported from {}",
                            export_name, import.library
                        ),
                        "",
                    ));
                }
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Top-level import parsing (for use at REPL / top-level)
// ---------------------------------------------------------------------------

/// Parse a top-level `(import <import-set> ...)` form.
pub fn parse_top_level_import(items: &[Value]) -> Result<Vec<ImportSet>, LispError> {
    // items[0] = "import"
    // items[1..] = import sets
    if items.len() < 2 {
        return Err(LispError::syntax(
            "import requires at least one import set",
            "",
        ));
    }
    let mut imports = Vec::new();
    for form in &items[1..] {
        imports.push(parse_import_set(form)?);
    }
    Ok(imports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_library_name_parse() {
        let val = Value::list(vec![Value::symbol("scheme"), Value::symbol("base")]);
        let name = LibraryName::from_value(&val).unwrap();
        assert_eq!(name.0, vec!["scheme", "base"]);
        assert_eq!(name.to_string(), "(scheme base)");
    }

    #[test]
    fn test_library_name_with_version() {
        let val = Value::list(vec![Value::symbol("srfi"), Value::Int(1)]);
        let name = LibraryName::from_value(&val).unwrap();
        assert_eq!(name.0, vec!["srfi", "1"]);
    }

    #[test]
    fn test_parse_export_simple() {
        let mut exports = HashMap::new();
        parse_export_specs(&[Value::symbol("foo"), Value::symbol("bar")], &mut exports).unwrap();
        assert_eq!(exports.get("foo"), Some(&"foo".to_string()));
        assert_eq!(exports.get("bar"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_parse_export_rename() {
        let mut exports = HashMap::new();
        parse_export_specs(
            &[Value::list(vec![
                Value::symbol("rename"),
                Value::symbol("internal-fn"),
                Value::symbol("public-fn"),
            ])],
            &mut exports,
        )
        .unwrap();
        assert_eq!(exports.get("public-fn"), Some(&"internal-fn".to_string()));
    }

    #[test]
    fn test_parse_import_simple() {
        let form = Value::list(vec![Value::symbol("scheme"), Value::symbol("base")]);
        let import = parse_import_set(&form).unwrap();
        assert_eq!(import.library.0, vec!["scheme", "base"]);
        assert!(matches!(import.bindings, ImportBindings::All));
    }

    #[test]
    fn test_parse_import_only() {
        let form = Value::list(vec![
            Value::symbol("only"),
            Value::list(vec![Value::symbol("scheme"), Value::symbol("base")]),
            Value::symbol("map"),
            Value::symbol("filter"),
        ]);
        let import = parse_import_set(&form).unwrap();
        assert_eq!(import.library.0, vec!["scheme", "base"]);
        if let ImportBindings::Explicit(map) = &import.bindings {
            assert_eq!(map.len(), 2);
            assert_eq!(map.get("map"), Some(&"map".to_string()));
            assert_eq!(map.get("filter"), Some(&"filter".to_string()));
        } else {
            panic!("expected Explicit bindings");
        }
    }

    #[test]
    fn test_parse_import_rename() {
        let form = Value::list(vec![
            Value::symbol("rename"),
            Value::list(vec![
                Value::symbol("only"),
                Value::list(vec![Value::symbol("scheme"), Value::symbol("base")]),
                Value::symbol("car"),
            ]),
            Value::list(vec![Value::symbol("car"), Value::symbol("first")]),
        ]);
        let import = parse_import_set(&form).unwrap();
        if let ImportBindings::Explicit(map) = &import.bindings {
            assert_eq!(map.get("first"), Some(&"car".to_string()));
            assert!(!map.contains_key("car"));
        } else {
            panic!("expected Explicit bindings");
        }
    }

    #[test]
    fn test_parse_import_prefix() {
        let form = Value::list(vec![
            Value::symbol("prefix"),
            Value::list(vec![
                Value::symbol("only"),
                Value::list(vec![Value::symbol("scheme"), Value::symbol("base")]),
                Value::symbol("car"),
                Value::symbol("cdr"),
            ]),
            Value::symbol("s:"),
        ]);
        let import = parse_import_set(&form).unwrap();
        if let ImportBindings::Explicit(map) = &import.bindings {
            assert_eq!(map.get("s:car"), Some(&"car".to_string()));
            assert_eq!(map.get("s:cdr"), Some(&"cdr".to_string()));
        } else {
            panic!("expected Explicit bindings");
        }
    }

    #[test]
    fn test_parse_define_library() {
        let code = "(define-library (test lib)
                      (export foo bar)
                      (import (scheme base))
                      (begin
                        (define foo 1)
                        (define bar 2)))";
        let reader = crate::reader::read_all(code).unwrap();
        let items = reader[0].to_vec().unwrap();
        let lib_def = parse_define_library(&items).unwrap();

        assert_eq!(lib_def.name.0, vec!["test", "lib"]);
        assert_eq!(lib_def.exports.len(), 2);
        assert_eq!(lib_def.imports.len(), 1);
        assert_eq!(lib_def.imports[0].library.0, vec!["scheme", "base"]);
        assert_eq!(lib_def.body.len(), 2);
    }

    #[test]
    fn test_resolve_import_all() {
        let lib = Library {
            name: LibraryName(vec!["test".into()]),
            exports: HashMap::from([("a".into(), Value::Int(1)), ("b".into(), Value::Int(2))]),
        };
        let import = ImportSet {
            library: lib.name.clone(),
            bindings: ImportBindings::All,
        };
        let resolved = resolve_import(&import, &lib).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved.get("a"), Some(&Value::Int(1)));
    }

    #[test]
    fn test_resolve_import_explicit() {
        let lib = Library {
            name: LibraryName(vec!["test".into()]),
            exports: HashMap::from([
                ("a".into(), Value::Int(1)),
                ("b".into(), Value::Int(2)),
                ("c".into(), Value::Int(3)),
            ]),
        };
        let import = ImportSet {
            library: lib.name.clone(),
            bindings: ImportBindings::Explicit(HashMap::from([
                ("x".into(), "a".into()),
                ("y".into(), "c".into()),
            ])),
        };
        let resolved = resolve_import(&import, &lib).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved.get("x"), Some(&Value::Int(1)));
        assert_eq!(resolved.get("y"), Some(&Value::Int(3)));
    }

    #[test]
    fn test_resolve_import_missing_export() {
        let lib = Library {
            name: LibraryName(vec!["test".into()]),
            exports: HashMap::from([("a".into(), Value::Int(1))]),
        };
        let import = ImportSet {
            library: lib.name.clone(),
            bindings: ImportBindings::Explicit(HashMap::from([("x".into(), "missing".into())])),
        };
        assert!(resolve_import(&import, &lib).is_err());
    }

    #[test]
    fn test_registry() {
        let mut reg = LibraryRegistry::new();
        let lib = Library {
            name: LibraryName(vec!["test".into(), "lib".into()]),
            exports: HashMap::new(),
        };
        reg.register(lib);
        assert!(reg.contains(&LibraryName(vec!["test".into(), "lib".into()])));
        assert!(!reg.contains(&LibraryName(vec!["other".into()])));
    }
}
