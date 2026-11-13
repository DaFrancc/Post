//! The JSON outputs: `openrpc.generated.json`, the API as a reviewable file,
//! and `fixtures.generated.json`, one sample of each type's JSON form that
//! the Rust and Dart fixtures tests hold serde and the Dart classes to.

use crate::Config;
use crate::model::{ApiModel, Field, Lit, ParamDefault, Ty, TypeDef, TypeKind, summary};
use crate::names::serde_snake;

/// A JSON value whose objects keep the order they are built in.
#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(u64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

fn s(v: &str) -> J {
    J::Str(v.to_owned())
}

fn obj(entries: Vec<(&str, J)>) -> J {
    J::Obj(entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

impl J {
    /// Two-space indented, a trailing newline.
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String, indent: usize) {
        let pad = |out: &mut String, n: usize| out.push_str(&"  ".repeat(n));
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Num(n) => out.push_str(&n.to_string()),
            J::Str(v) => out.push_str(&serde_json::to_string(v).expect("a string encodes")),
            J::Arr(items) if items.is_empty() => out.push_str("[]"),
            J::Arr(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(out, indent + 1);
                    item.write(out, indent + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                pad(out, indent);
                out.push(']');
            }
            J::Obj(entries) if entries.is_empty() => out.push_str("{}"),
            J::Obj(entries) => {
                out.push_str("{\n");
                for (i, (k, v)) in entries.iter().enumerate() {
                    pad(out, indent + 1);
                    out.push_str(&serde_json::to_string(k).expect("a string encodes"));
                    out.push_str(": ");
                    v.write(out, indent + 1);
                    out.push_str(if i + 1 < entries.len() { ",\n" } else { "\n" });
                }
                pad(out, indent);
                out.push('}');
            }
        }
    }
}

// ---- OpenRPC -------------------------------------------------------------

/// A type's schema, as object entries so a caller can add to them.
fn schema(ty: &Ty, unix_ms: bool) -> Vec<(String, J)> {
    let e = |entries: Vec<(&str, J)>| entries.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    match ty {
        Ty::Unit => e(vec![("type", s("null"))]),
        Ty::Bool => e(vec![("type", s("boolean"))]),
        Ty::U32 => e(vec![("type", s("integer")), ("minimum", J::Num(0)), ("maximum", J::Num(u32::MAX.into()))]),
        Ty::U64 if unix_ms => e(vec![("type", s("integer")), ("minimum", J::Num(0)), ("format", s("unix-ms"))]),
        Ty::U64 => e(vec![("type", s("integer")), ("minimum", J::Num(0))]),
        Ty::String => e(vec![("type", s("string"))]),
        Ty::Id(_) => e(vec![("type", s("string")), ("pattern", s("^[0-9a-f]{32}$"))]),
        Ty::Named(n) => e(vec![("$ref", s(&format!("#/components/schemas/{n}")))]),
        Ty::Option(t) => e(vec![("oneOf", J::Arr(vec![J::Obj(schema(t, unix_ms)), obj(vec![("type", s("null"))])]))]),
        Ty::Vec(t) => e(vec![("type", s("array")), ("items", J::Obj(schema(t, unix_ms)))]),
    }
}

/// A schema with a description. Beside a `$ref` other keys are ignored, so a
/// reference is wrapped in `allOf`.
fn described(docs: &str, entries: Vec<(String, J)>) -> J {
    let mut out = vec![("description".to_owned(), s(docs))];
    if entries.iter().any(|(k, _)| k == "$ref") {
        out.push(("allOf".to_owned(), J::Arr(vec![J::Obj(entries)])));
    } else {
        out.extend(entries);
    }
    J::Obj(out)
}

fn required(f: &Field) -> bool {
    !f.default && !matches!(f.ty, Ty::Option(_))
}

fn object_schema(docs: &str, tag: Option<&str>, fields: &[Field]) -> J {
    let mut props = Vec::new();
    let mut req = Vec::new();
    if let Some(tag) = tag {
        props.push(("type".to_owned(), obj(vec![("const", s(tag))])));
        req.push(s("type"));
    }
    for f in fields {
        props.push((f.name.clone(), described(&f.docs, schema(&f.ty, f.unix_ms))));
        if required(f) {
            req.push(s(&f.name));
        }
    }
    obj(vec![
        ("description", s(docs)),
        ("type", s("object")),
        ("properties", J::Obj(props)),
        ("required", J::Arr(req)),
        ("additionalProperties", J::Bool(false)),
    ])
}

fn type_schema(t: &TypeDef) -> J {
    match &t.kind {
        TypeKind::Struct(fields) => object_schema(&t.docs, None, fields),
        TypeKind::UnitEnum(variants) => obj(vec![
            ("description", s(&t.docs)),
            (
                "oneOf",
                J::Arr(variants.iter().map(|v| obj(vec![("const", s(&serde_snake(&v.name))), ("description", s(&v.docs))])).collect()),
            ),
        ]),
        TypeKind::TaggedEnum(variants) => obj(vec![
            ("description", s(&t.docs)),
            ("oneOf", J::Arr(variants.iter().map(|v| object_schema(&v.docs, Some(&serde_snake(&v.name)), &v.fields)).collect())),
        ]),
    }
}

fn lit(l: &Lit) -> J {
    match l {
        Lit::Bool(b) => J::Bool(*b),
        Lit::Int(n) => J::Num(*n),
        Lit::Str(v) => s(v),
    }
}

/// `openrpc.generated.json`.
pub fn openrpc(model: &ApiModel, cfg: &Config) -> String {
    let methods = model.methods.iter().map(|m| {
        let params = m.params.iter().map(|p| {
            let mut sch = schema(&p.ty, p.unix_ms);
            match &p.default {
                ParamDefault::Value(l) => sch.push(("default".to_owned(), lit(l))),
                ParamDefault::Type => sch.push((
                    "default".to_owned(),
                    match &p.ty {
                        Ty::Bool => J::Bool(false),
                        Ty::String => s(""),
                        Ty::Vec(_) => J::Arr(Vec::new()),
                        _ => J::Num(0),
                    },
                )),
                ParamDefault::None => {}
            }
            obj(vec![
                ("name", s(&p.name)),
                ("description", s(&p.docs)),
                ("required", J::Bool(p.default == ParamDefault::None && !matches!(p.ty, Ty::Option(_)))),
                ("schema", J::Obj(sch)),
            ])
        });
        obj(vec![
            ("name", s(&m.name)),
            ("summary", s(&summary(&m.docs))),
            ("description", s(&m.docs)),
            ("x-needs", s(m.needs.as_str())),
            ("paramStructure", s("by-name")),
            ("params", J::Arr(params.collect())),
            ("result", obj(vec![("name", s("result")), ("schema", J::Obj(schema(&m.ret, false)))])),
        ])
    });
    let schemas = model.types.iter().map(|t| (t.name.clone(), type_schema(t))).collect();
    obj(vec![
        ("x-generated", s(&cfg.header)),
        ("openrpc", s("1.3.2")),
        ("info", obj(vec![("title", s(&cfg.title)), ("version", s(&cfg.version))])),
        ("methods", J::Arr(methods.collect())),
        ("components", obj(vec![("schemas", J::Obj(schemas))])),
    ])
    .render()
}

// ---- fixtures ------------------------------------------------------------

/// The sample ID: 16 bytes, as the core writes them.
const SAMPLE_ID: &str = "00112233445566778899aabbccddeeff";

/// Builds samples. `full` gives every `Option` a value; otherwise each is
/// null. `stack` stops a type that lists itself from recursing forever.
struct Samples<'a> {
    model: &'a ApiModel,
    stack: Vec<String>,
}

impl Samples<'_> {
    fn value(&mut self, ty: &Ty, full: bool) -> J {
        match ty {
            Ty::Unit => J::Null,
            Ty::Bool => J::Bool(true),
            Ty::U32 | Ty::U64 => J::Num(1),
            Ty::String => s("s"),
            Ty::Id(_) => s(SAMPLE_ID),
            Ty::Option(t) => {
                if full {
                    self.value(t, full)
                } else {
                    J::Null
                }
            }
            Ty::Vec(t) => match &**t {
                Ty::Named(n) if self.stack.contains(n) => J::Arr(Vec::new()),
                t => J::Arr(vec![self.value(t, full)]),
            },
            Ty::Named(n) => {
                let Some(def) = self.model.ty(n) else { return J::Null };
                if self.stack.contains(n) {
                    return J::Null;
                }
                self.stack.push(n.clone());
                let v = match &def.kind {
                    TypeKind::Struct(fields) => self.object(None, fields, full),
                    TypeKind::UnitEnum(vs) => vs.first().map_or(J::Null, |v| s(&serde_snake(&v.name))),
                    TypeKind::TaggedEnum(vs) => vs.first().map_or(J::Null, |v| self.object(Some(&serde_snake(&v.name)), &v.fields, full)),
                };
                self.stack.pop();
                v
            }
        }
    }

    fn object(&mut self, tag: Option<&str>, fields: &[Field], full: bool) -> J {
        let mut entries = Vec::new();
        if let Some(tag) = tag {
            entries.push(("type".to_owned(), s(tag)));
        }
        for f in fields {
            entries.push((f.name.clone(), self.value(&f.ty, full)));
        }
        J::Obj(entries)
    }

    /// Every sample of one type: full, then (if different) with every
    /// Option null; per variant for an enum.
    fn of(&mut self, t: &TypeDef) -> Vec<J> {
        self.stack.push(t.name.clone());
        let mut out = Vec::new();
        match &t.kind {
            TypeKind::Struct(fields) => self.full_and_bare(&mut out, None, fields),
            TypeKind::UnitEnum(vs) => out.extend(vs.iter().map(|v| s(&serde_snake(&v.name)))),
            TypeKind::TaggedEnum(vs) => {
                for v in vs {
                    self.full_and_bare(&mut out, Some(&serde_snake(&v.name)), &v.fields);
                }
            }
        }
        self.stack.pop();
        out
    }

    /// The object with every Option set, then with every Option null if
    /// that differs.
    fn full_and_bare(&mut self, out: &mut Vec<J>, tag: Option<&str>, fields: &[Field]) {
        let full = self.object(tag, fields, true);
        let bare = self.object(tag, fields, false);
        out.push(full.clone());
        if bare != full {
            out.push(bare);
        }
    }
}

/// `fixtures.generated.json`: `{ "<Type>": [samples…] }`.
pub fn fixtures(model: &ApiModel, cfg: &Config) -> String {
    let mut samples = Samples { model, stack: Vec::new() };
    let mut entries = vec![("x-generated".to_owned(), s(&cfg.header))];
    for t in &model.types {
        entries.push((t.name.clone(), J::Arr(samples.of(t))));
    }
    J::Obj(entries).render()
}
