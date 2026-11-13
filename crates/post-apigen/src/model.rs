//! What the generator knows about the API, read from the annotated source.

use std::fmt;

/// Where something was declared, for error messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loc {
    /// The source file, as given to the parser.
    pub file: String,
    /// 1-based.
    pub line: usize,
    /// 1-based.
    pub column: usize,
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// The whole API: types, then methods, each in source order.
#[derive(Clone, Debug)]
pub struct ApiModel {
    /// Every `#[api]` struct and enum.
    pub types: Vec<TypeDef>,
    /// Every `#[api]` method on `Core`.
    pub methods: Vec<Method>,
}

impl ApiModel {
    /// The API type called `name`.
    pub fn ty(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name == name)
    }
}

/// An `#[api]` struct or enum.
#[derive(Clone, Debug)]
pub struct TypeDef {
    /// Its Rust name.
    pub name: String,
    /// Its public path in the core crate, without the crate: `Fill`, or
    /// `picture::PictureKind`.
    pub path: String,
    /// Its doc comment.
    pub docs: String,
    /// Its shape.
    pub kind: TypeKind,
    /// Where it is declared.
    pub loc: Loc,
}

/// The shape of an API type.
#[derive(Clone, Debug)]
pub enum TypeKind {
    /// A struct with named fields: a JSON object.
    Struct(Vec<Field>),
    /// An enum whose variants are all unit: a JSON string.
    UnitEnum(Vec<Variant>),
    /// An enum with fields: a JSON object tagged with `"type"`.
    TaggedEnum(Vec<Variant>),
}

/// An enum variant.
#[derive(Clone, Debug)]
pub struct Variant {
    /// Its Rust name.
    pub name: String,
    /// Its doc comment.
    pub docs: String,
    /// Its named fields; none for a unit variant.
    pub fields: Vec<Field>,
    /// Where it is declared.
    pub loc: Loc,
}

/// A struct or variant field.
#[derive(Clone, Debug)]
pub struct Field {
    /// Its Rust name, which is its JSON key.
    pub name: String,
    /// Its doc comment.
    pub docs: String,
    /// Its type.
    pub ty: Ty,
    /// Marked `unix_ms`: a time.
    pub unix_ms: bool,
    /// Marked `default`: may be left out when read.
    pub default: bool,
    /// Where it is declared.
    pub loc: Loc,
}

/// A type the API can carry (docs/phase-1-api.md, "The type table").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// `()`, for returns only.
    Unit,
    /// `bool`.
    Bool,
    /// `u32`.
    U32,
    /// `u64`.
    U64,
    /// `String` (or `&str` as a parameter).
    String,
    /// One of the core's hex IDs, by name.
    Id(String),
    /// An `#[api]` type, by name.
    Named(String),
    /// `Option<T>`.
    Option(Box<Ty>),
    /// `Vec<T>` (or `&[T]` as a parameter).
    Vec(Box<Ty>),
}

impl Ty {
    /// Whether a `Vec` is anywhere inside.
    pub fn has_list(&self) -> bool {
        match self {
            Ty::Vec(_) => true,
            Ty::Option(t) => t.has_list(),
            _ => false,
        }
    }
}

/// The core's hex ID types: 16 bytes written as 32 hex characters.
pub const ID_TYPES: &[&str] = &["ConversationId", "MessageId", "CallId"];

/// What a method needs before it can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Works offline, from local data.
    Local,
    /// Needs the account and a connection.
    Connection,
    /// Creates the account; needs no prior connection.
    Registration,
}

impl Needs {
    /// Its name in `#[api(needs = …)]` and in the OpenRPC file.
    pub fn as_str(self) -> &'static str {
        match self {
            Needs::Local => "local",
            Needs::Connection => "connection",
            Needs::Registration => "registration",
        }
    }
}

/// An `#[api]` method on `Core`.
#[derive(Clone, Debug)]
pub struct Method {
    /// Its Rust name, which is its API name.
    pub name: String,
    /// Its doc comment.
    pub docs: String,
    /// What it needs first.
    pub needs: Needs,
    /// Declared `async`.
    pub is_async: bool,
    /// Returns `Result<…>`.
    pub fallible: bool,
    /// What it returns (inside the `Result`, if any).
    pub ret: Ty,
    /// Its parameters after `&self`.
    pub params: Vec<Param>,
    /// Where it is declared.
    pub loc: Loc,
}

/// A method parameter.
#[derive(Clone, Debug)]
pub struct Param {
    /// Its Rust name, which is its JSON key.
    pub name: String,
    /// Its doc comment.
    pub docs: String,
    /// Its type in the params struct (owned: `&str` is `String`).
    pub ty: Ty,
    /// How the params struct's field is passed to the method.
    pub pass: Pass,
    /// Marked `unix_ms`: a time.
    pub unix_ms: bool,
    /// What it is when left out.
    pub default: ParamDefault,
    /// Where it is declared.
    pub loc: Loc,
}

/// How a params struct field is handed to the core method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// By value: `p.x`.
    Value,
    /// Borrowed: `&p.x` (for `&str`, `&[T]`, `&T`).
    Ref,
    /// `p.x.as_deref()` (for `Option<&str>`, `Option<&[T]>`).
    OptionDeref,
    /// `p.x.as_ref()` (for `Option<&T>`).
    OptionRef,
}

/// A parameter's value when it is left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamDefault {
    /// Required (an `Option` may still be left out, as `None`).
    None,
    /// `default`: `Default::default()`.
    Type,
    /// `default = <literal>`.
    Value(Lit),
}

/// A literal given as a parameter's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lit {
    /// `true` or `false`.
    Bool(bool),
    /// A non-negative integer.
    Int(u64),
    /// A string.
    Str(String),
}

/// The first paragraph of a doc comment, its whitespace collapsed.
pub fn summary(docs: &str) -> String {
    let para = docs.split("\n\n").next().unwrap_or("");
    para.split_whitespace().collect::<Vec<_>>().join(" ")
}
