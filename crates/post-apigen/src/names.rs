//! How Rust names become JSON keys and Dart names.

/// A variant's JSON name, as serde's `rename_all = "snake_case"` writes it.
pub fn serde_snake(variant: &str) -> String {
    let mut out = String::new();
    for (i, ch) in variant.char_indices() {
        if i > 0 && ch.is_uppercase() {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// `this_device` → `thisDevice`.
pub fn lower_camel(snake: &str) -> String {
    let mut out = String::new();
    for (i, part) in snake.split('_').filter(|p| !p.is_empty()).enumerate() {
        if i == 0 {
            out.push_str(part);
        } else {
            out.push_str(&capitalise(part));
        }
    }
    out
}

/// `set_admin` → `SetAdmin`.
pub fn upper_camel(snake: &str) -> String {
    snake.split('_').map(capitalise).collect()
}

/// `DoNotDisturb` → `doNotDisturb`.
pub fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A field's or parameter's Dart name: lowerCamelCase, and a time
/// (`unix_ms`) loses a trailing `_ms` (`expires_at_ms` → `expiresAt`).
pub fn dart_member(name: &str, unix_ms: bool) -> String {
    let base = if unix_ms { name.strip_suffix("_ms").unwrap_or(name) } else { name };
    lower_camel(base)
}

/// The Dart class of a tagged enum's variant: `Event::Presence` →
/// `PresenceEvent`, which does not clash with a `Presence` type.
pub fn dart_variant_class(variant: &str, enum_name: &str) -> String {
    format!("{variant}{enum_name}")
}

/// Words Dart does not allow as names, or that the generated code cannot use
/// as one (`required` reads as the modifier in a parameter list).
pub const DART_RESERVED: &[&str] = &[
    "assert", "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum", "extends",
    "false", "final", "finally", "for", "if", "in", "is", "late", "new", "null", "required", "rethrow", "return", "super", "switch",
    "this", "throw", "true", "try", "var", "void", "while", "with", "yield",
];

/// Members every Dart object has, which a field or method must not shadow.
pub const DART_OBJECT_MEMBERS: &[&str] = &["hashCode", "runtimeType", "toString", "noSuchMethod"];

/// Names the generated classes use for themselves.
pub const DART_GENERATED_MEMBERS: &[&str] = &["toJson", "fromJson"];

/// Members every Dart enum has, plus the generated `json`.
pub const DART_ENUM_MEMBERS: &[&str] = &["values", "index", "name", "json"];

/// Members of the generated `PostApi`.
pub const DART_API_MEMBERS: &[&str] = &["events"];

/// Type names the generated Dart file declares or relies on, which an API
/// type must not take.
pub const DART_TAKEN_TYPES: &[&str] = &[
    "PostApi",
    "RpcPostApi",
    "PostTransport",
    "Object",
    "String",
    "int",
    "double",
    "num",
    "bool",
    "List",
    "Map",
    "Set",
    "Iterable",
    "DateTime",
    "Duration",
    "Future",
    "Stream",
    "Function",
    "Type",
    "Symbol",
    "Null",
    "Never",
    "Record",
    "Enum",
    "Error",
    "Exception",
    "FormatException",
    "StateError",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(serde_snake("DoNotDisturb"), "do_not_disturb");
        assert_eq!(serde_snake("Lagged"), "lagged");
        assert_eq!(lower_camel("this_device"), "thisDevice");
        assert_eq!(lower_camel("id"), "id");
        assert_eq!(upper_camel("set_admin"), "SetAdmin");
        assert_eq!(lower_first("DoNotDisturb"), "doNotDisturb");
        assert_eq!(dart_member("expires_at_ms", true), "expiresAt");
        assert_eq!(dart_member("expires_at_ms", false), "expiresAtMs");
        assert_eq!(dart_variant_class("Presence", "Event"), "PresenceEvent");
    }
}
