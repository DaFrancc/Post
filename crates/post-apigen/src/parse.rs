//! Reading the annotated source into an [`ApiModel`], with the errors of
//! docs/phase-1-api.md, "Generator errors". Every problem is collected, each
//! with its file, line and column; nothing is guessed.

use std::collections::{HashMap, HashSet};
use std::fmt;

use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;

use crate::model::{ApiModel, Field, ID_TYPES, Lit, Loc, Method, Needs, Param, ParamDefault, Pass, Ty, TypeDef, TypeKind, Variant};
use crate::names;

/// One source file to read.
pub struct Source<'a> {
    /// How errors name it.
    pub path: &'a str,
    /// The public path of its items in the core crate: `""` for the crate
    /// root (or re-exported there), `"picture"`.
    pub module: &'a str,
    /// Its contents.
    pub text: &'a str,
}

/// A problem in the annotated source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenError {
    /// Where.
    pub loc: Loc,
    /// What, in a sentence.
    pub message: String,
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.loc, self.message)
    }
}

/// Every problem found, one per line.
#[derive(Debug)]
pub struct Errors(pub Vec<GenError>);

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, e) in self.0.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Errors {}

/// Read `sources`, in order, into the API they declare.
pub fn parse(sources: &[Source<'_>]) -> Result<ApiModel, Errors> {
    let mut cx = Cx { file: String::new(), errors: Vec::new(), uses: Vec::new() };
    let mut model = ApiModel { types: Vec::new(), methods: Vec::new() };
    for s in sources {
        cx.file = s.path.to_owned();
        match syn::parse_file(s.text) {
            Ok(file) => file.items.iter().for_each(|item| cx.item(item, s.module, &mut model)),
            Err(e) => cx.err(e.span(), e.to_string()),
        }
    }
    cx.check_uses(&model);
    cx.check_names(&model);
    if cx.errors.is_empty() { Ok(model) } else { Err(Errors(cx.errors)) }
}

/// A key inside `#[api(...)]`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    Needs(Needs),
    UnixMs,
    Default,
    DefaultValue(Lit),
}

impl Key {
    fn name(&self) -> &'static str {
        match self {
            Key::Needs(_) => "needs",
            Key::UnixMs => "unix_ms",
            Key::Default => "default",
            Key::DefaultValue(_) => "default = …",
        }
    }
}

/// Types that look like they might fit but are not in the table.
const NOT_IN_TABLE: &[&str] =
    &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u128", "usize", "f32", "f64", "char", "str", "Self"];

struct Cx {
    /// The file being read.
    file: String,
    errors: Vec<GenError>,
    /// Every type name used as an API type, checked once all are known.
    uses: Vec<(String, Loc)>,
}

fn is_api(a: &syn::Attribute) -> bool {
    let p = a.path();
    p.is_ident("api") || (p.segments.len() == 2 && p.segments[0].ident == "post_api_macros" && p.segments[1].ident == "api")
}

fn has_api(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(is_api)
}

/// A doc comment's text: one line per `///` line, the space after `///`
/// dropped, blank lines at either end trimmed.
fn docs(attrs: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for a in attrs.iter().filter(|a| a.path().is_ident("doc")) {
        if let syn::Meta::NameValue(nv) = &a.meta
            && let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value
        {
            for line in s.value().split('\n') {
                lines.push(line.strip_prefix(' ').unwrap_or(line).trim_end().to_owned());
            }
        }
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let first = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    lines[first..].join("\n")
}

/// The type arguments of a path segment: `[]` for none, `None` if any is not
/// a type (a lifetime, a constant) or the arguments are `Fn(…)`-style.
fn type_args(seg: &syn::PathSegment) -> Option<Vec<&syn::Type>> {
    match &seg.arguments {
        syn::PathArguments::None => Some(Vec::new()),
        syn::PathArguments::AngleBracketed(a) => {
            a.args.iter().map(|g| if let syn::GenericArgument::Type(t) = g { Some(t) } else { None }).collect()
        }
        syn::PathArguments::Parenthesized(_) => None,
    }
}

/// A type as written, for messages.
fn show(ty: &syn::Type) -> String {
    ty.to_token_stream().to_string().replace(" <", "<").replace("< ", "<").replace(" >", ">").replace("& ", "&").replace(" ,", ",")
}

fn join_path(module: &str, name: &str) -> String {
    if module.is_empty() { name.to_owned() } else { format!("{module}::{name}") }
}

impl Cx {
    fn loc(&self, span: Span) -> Loc {
        let start = span.start();
        Loc { file: self.file.clone(), line: start.line, column: start.column + 1 }
    }

    fn err(&mut self, span: Span, message: impl Into<String>) {
        let loc = self.loc(span);
        self.errors.push(GenError { loc, message: message.into() });
    }

    fn err_at(&mut self, loc: &Loc, message: impl Into<String>) {
        self.errors.push(GenError { loc: loc.clone(), message: message.into() });
    }

    // ---- items -----------------------------------------------------------

    fn item(&mut self, item: &syn::Item, module: &str, model: &mut ApiModel) {
        match item {
            syn::Item::Struct(s) if has_api(&s.attrs) => {
                if let Some(t) = self.structure(s, module) {
                    model.types.push(t);
                }
            }
            syn::Item::Enum(e) if has_api(&e.attrs) => {
                if let Some(t) = self.enumeration(e, module) {
                    model.types.push(t);
                }
            }
            syn::Item::Impl(i) => self.implementation(i, model),
            other => {
                let attrs: &[syn::Attribute] = match other {
                    syn::Item::Fn(x) => &x.attrs,
                    syn::Item::Const(x) => &x.attrs,
                    syn::Item::Static(x) => &x.attrs,
                    syn::Item::Type(x) => &x.attrs,
                    syn::Item::Trait(x) => &x.attrs,
                    syn::Item::Mod(x) => &x.attrs,
                    syn::Item::Union(x) => &x.attrs,
                    syn::Item::Use(x) => &x.attrs,
                    _ => &[],
                };
                if let Some(a) = attrs.iter().find(|a| is_api(a)) {
                    self.err(a.span(), "#[api] goes on a struct, an enum, or a method in `impl Core`");
                }
            }
        }
    }

    fn implementation(&mut self, i: &syn::ItemImpl, model: &mut ApiModel) {
        let is_core = i.trait_.is_none()
            && i.generics.params.is_empty()
            && matches!(&*i.self_ty, syn::Type::Path(p) if p.qself.is_none() && p.path.is_ident("Core"));
        for it in &i.items {
            let attrs: &[syn::Attribute] = match it {
                syn::ImplItem::Fn(f) => &f.attrs,
                syn::ImplItem::Const(c) => &c.attrs,
                syn::ImplItem::Type(t) => &t.attrs,
                syn::ImplItem::Macro(m) => &m.attrs,
                _ => continue,
            };
            let Some(a) = attrs.iter().find(|a| is_api(a)) else { continue };
            match it {
                syn::ImplItem::Fn(f) if is_core => {
                    if let Some(m) = self.method(f) {
                        model.methods.push(m);
                    }
                }
                syn::ImplItem::Fn(_) => self.err(a.span(), "an #[api] method must be in an inherent `impl Core` block"),
                _ => self.err(a.span(), "#[api] goes on a struct, an enum, or a method in `impl Core`"),
            }
        }
    }

    fn structure(&mut self, s: &syn::ItemStruct, module: &str) -> Option<TypeDef> {
        let name = self.name_of(&s.ident)?;
        let keys = self.keys(&s.attrs);
        self.allow(&keys, "a type", &[]);
        let docs = self.need_docs(&s.attrs, s.ident.span(), &format!("`{name}`"));
        self.no_generics(&s.generics, &name);
        let syn::Fields::Named(fields) = &s.fields else {
            self.err(s.ident.span(), format!("`{name}` is a tuple or unit struct: an API type has named fields"));
            return None;
        };
        let fields = fields.named.iter().filter_map(|f| self.field(f, &name, true)).collect();
        Some(TypeDef { path: join_path(module, &name), docs, kind: TypeKind::Struct(fields), loc: self.loc(s.ident.span()), name })
    }

    fn enumeration(&mut self, e: &syn::ItemEnum, module: &str) -> Option<TypeDef> {
        let name = self.name_of(&e.ident)?;
        let keys = self.keys(&e.attrs);
        self.allow(&keys, "a type", &[]);
        let docs = self.need_docs(&e.attrs, e.ident.span(), &format!("`{name}`"));
        self.no_generics(&e.generics, &name);
        // As the macro decides: a string only if every variant is unit.
        let unit_only = e.variants.iter().all(|v| matches!(v.fields, syn::Fields::Unit));
        let mut variants = Vec::new();
        for v in &e.variants {
            let Some(vname) = self.name_of(&v.ident) else { continue };
            let owner = format!("{name}::{vname}");
            let keys = self.keys(&v.attrs);
            self.allow(&keys, "a variant", &[]);
            let docs = self.need_docs(&v.attrs, v.ident.span(), &format!("variant `{owner}`"));
            let fields = match &v.fields {
                syn::Fields::Unit => Vec::new(),
                syn::Fields::Named(n) => n.named.iter().filter_map(|f| self.field(f, &owner, false)).collect(),
                syn::Fields::Unnamed(_) => {
                    self.err(v.ident.span(), format!("`{owner}` is a tuple variant: give it named fields"));
                    continue;
                }
            };
            variants.push(Variant { name: vname, docs, fields, loc: self.loc(v.ident.span()) });
        }
        let kind = if unit_only { TypeKind::UnitEnum(variants) } else { TypeKind::TaggedEnum(variants) };
        Some(TypeDef { path: join_path(module, &name), docs, kind, loc: self.loc(e.ident.span()), name })
    }

    fn field(&mut self, f: &syn::Field, owner: &str, must_be_pub: bool) -> Option<Field> {
        let ident = f.ident.as_ref()?;
        let name = self.name_of(ident)?;
        if must_be_pub && !matches!(f.vis, syn::Visibility::Public(_)) {
            self.err(ident.span(), format!("field `{name}` of `{owner}` must be pub"));
        }
        let docs = self.need_docs(&f.attrs, ident.span(), &format!("field `{name}` of `{owner}`"));
        let keys = self.keys(&f.attrs);
        self.allow(&keys, "a field", &["unix_ms", "default"]);
        let unix_ms = keys.iter().any(|(k, _)| *k == Key::UnixMs);
        let default = keys.iter().any(|(k, _)| *k == Key::Default);
        let ty = self.owned_ty(&f.ty)?;
        self.check_unix_ms(&ty, unix_ms, f.ty.span());
        if default {
            self.check_type_default(&ty, unix_ms, f.ty.span());
        }
        Some(Field { name, docs, ty, unix_ms, default, loc: self.loc(ident.span()) })
    }

    fn method(&mut self, f: &syn::ImplItemFn) -> Option<Method> {
        let sig = &f.sig;
        let name = self.name_of(&sig.ident)?;
        let span = sig.ident.span();
        if !matches!(f.vis, syn::Visibility::Public(_)) {
            self.err(span, format!("`{name}` must be pub to be in the API"));
        }
        if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
            self.err(span, format!("`{name}` is generic: an API method cannot be"));
        }
        let keys = self.keys(&f.attrs);
        self.allow(&keys, "a method", &["needs"]);
        let needs = keys.iter().find_map(|(k, _)| if let Key::Needs(n) = k { Some(*n) } else { None });
        // A `needs` that did not parse has been reported already.
        let tried = f.attrs.iter().filter(|a| is_api(a)).any(|a| a.meta.to_token_stream().to_string().contains("needs"));
        if needs.is_none() && !tried {
            self.err(span, format!("`{name}` needs `#[api(needs = local | connection | registration)]`"));
        }
        let docs = self.need_docs(&f.attrs, span, &format!("method `{name}`"));
        if !matches!(sig.receiver(), Some(r) if r.reference.is_some() && r.mutability.is_none()) {
            self.err(span, format!("`{name}` must take `&self`"));
        }
        let params: Vec<Param> = sig
            .inputs
            .iter()
            .filter_map(|a| if let syn::FnArg::Typed(t) = a { Some(t) } else { None })
            .filter_map(|t| self.param(t, &name))
            .collect();
        let (fallible, ret) = match &sig.output {
            syn::ReturnType::Default => (false, Ty::Unit),
            syn::ReturnType::Type(_, ty) => self.return_ty(ty)?,
        };
        Some(Method { name, docs, needs: needs?, is_async: sig.asyncness.is_some(), fallible, ret, params, loc: self.loc(span) })
    }

    fn param(&mut self, t: &syn::PatType, method: &str) -> Option<Param> {
        let syn::Pat::Ident(pi) = &*t.pat else {
            self.err(t.pat.span(), format!("a parameter of `{method}` must be a plain name"));
            return None;
        };
        if pi.subpat.is_some() || pi.by_ref.is_some() {
            self.err(t.pat.span(), format!("a parameter of `{method}` must be a plain name"));
        }
        let name = self.name_of(&pi.ident)?;
        let docs = self.need_docs(&t.attrs, pi.ident.span(), &format!("parameter `{name}` of `{method}`"));
        let keys = self.keys(&t.attrs);
        self.allow(&keys, "a parameter", &["unix_ms", "default", "default = …"]);
        let unix_ms = keys.iter().any(|(k, _)| *k == Key::UnixMs);
        let defaults: Vec<ParamDefault> = keys
            .iter()
            .filter_map(|(k, _)| match k {
                Key::Default => Some(ParamDefault::Type),
                Key::DefaultValue(l) => Some(ParamDefault::Value(l.clone())),
                _ => None,
            })
            .collect();
        if defaults.len() > 1 {
            self.err(pi.ident.span(), format!("parameter `{name}` of `{method}` has more than one default"));
        }
        let default = defaults.into_iter().next().unwrap_or(ParamDefault::None);
        let (ty, pass) = self.param_ty(&t.ty)?;
        self.check_unix_ms(&ty, unix_ms, t.ty.span());
        match &default {
            ParamDefault::None => {}
            ParamDefault::Type => self.check_type_default(&ty, unix_ms, t.ty.span()),
            ParamDefault::Value(lit) => self.check_lit(&ty, lit, unix_ms, t.ty.span()),
        }
        Some(Param { name, docs, ty, pass, unix_ms, default, loc: self.loc(pi.ident.span()) })
    }

    // ---- annotations -----------------------------------------------------

    /// The keys of every `#[api(...)]` among `attrs`.
    fn keys(&mut self, attrs: &[syn::Attribute]) -> Vec<(Key, Span)> {
        let mut out = Vec::new();
        for a in attrs.iter().filter(|a| is_api(a)) {
            match &a.meta {
                syn::Meta::Path(_) => {}
                syn::Meta::List(_) => {
                    let parsed = a.parse_nested_meta(|m| {
                        let span = m.path.span();
                        let key = m.path.get_ident().map(ToString::to_string).unwrap_or_default();
                        match key.as_str() {
                            "needs" => {
                                let v: syn::Ident = m.value()?.parse()?;
                                let needs = match v.to_string().as_str() {
                                    "local" => Needs::Local,
                                    "connection" => Needs::Connection,
                                    "registration" => Needs::Registration,
                                    other => {
                                        return Err(syn::Error::new(
                                            v.span(),
                                            format!("unknown `needs = {other}`: local, connection or registration"),
                                        ));
                                    }
                                };
                                out.push((Key::Needs(needs), span));
                            }
                            "unix_ms" => {
                                if m.input.peek(syn::Token![=]) {
                                    return Err(m.error("`unix_ms` takes no value"));
                                }
                                out.push((Key::UnixMs, span));
                            }
                            "default" if m.input.peek(syn::Token![=]) => {
                                let lit = match m.value()?.parse::<syn::Lit>()? {
                                    syn::Lit::Bool(b) => Lit::Bool(b.value),
                                    syn::Lit::Int(i) => Lit::Int(i.base10_parse()?),
                                    syn::Lit::Str(s) => Lit::Str(s.value()),
                                    other => {
                                        return Err(syn::Error::new(
                                            other.span(),
                                            "a default is a bool, a non-negative integer or a string",
                                        ));
                                    }
                                };
                                out.push((Key::DefaultValue(lit), span));
                            }
                            "default" => out.push((Key::Default, span)),
                            _ => return Err(m.error(format!("unknown key `{}`", m.path.to_token_stream()))),
                        }
                        Ok(())
                    });
                    if let Err(e) = parsed {
                        self.err(e.span(), e.to_string());
                    }
                }
                syn::Meta::NameValue(nv) => self.err(nv.span(), "write #[api] or #[api(key, …)]"),
            }
        }
        out
    }

    /// Report every key not in `allowed` for this kind of item.
    fn allow(&mut self, keys: &[(Key, Span)], site: &str, allowed: &[&str]) {
        for (k, span) in keys {
            if !allowed.contains(&k.name()) {
                self.err(*span, format!("`{}` does not apply to {site}", k.name()));
            }
        }
    }

    fn need_docs(&mut self, attrs: &[syn::Attribute], span: Span, what: &str) -> String {
        let d = docs(attrs);
        if d.is_empty() {
            self.err(span, format!("{what} needs a doc comment"));
        }
        d
    }

    fn no_generics(&mut self, g: &syn::Generics, name: &str) {
        if !g.params.is_empty() {
            self.err(g.span(), format!("`{name}` is generic: an API type cannot be"));
        }
    }

    fn name_of(&mut self, ident: &syn::Ident) -> Option<String> {
        let s = ident.to_string();
        if s.starts_with("r#") {
            self.err(ident.span(), format!("`{s}` is a raw identifier: give it a plain name"));
            return None;
        }
        Some(s)
    }

    fn check_unix_ms(&mut self, ty: &Ty, unix_ms: bool, span: Span) {
        if unix_ms && *ty != Ty::U64 && *ty != Ty::Option(Box::new(Ty::U64)) {
            self.err(span, "`unix_ms` goes on a u64 or an Option<u64>");
        }
    }

    fn check_type_default(&mut self, ty: &Ty, unix_ms: bool, span: Span) {
        if unix_ms {
            self.err(span, "a time (`unix_ms`) cannot have a default");
        } else if matches!(ty, Ty::Option(_)) {
            self.err(span, "an Option is already optional: drop `default`");
        } else if !matches!(ty, Ty::Bool | Ty::U32 | Ty::U64 | Ty::String | Ty::Vec(_)) {
            self.err(span, "`default` goes on a bool, an integer, a String or a list");
        }
    }

    fn check_lit(&mut self, ty: &Ty, lit: &Lit, unix_ms: bool, span: Span) {
        let fits = match (ty, lit) {
            (Ty::Bool, Lit::Bool(_)) | (Ty::U64, Lit::Int(_)) | (Ty::String, Lit::Str(_)) => true,
            (Ty::U32, Lit::Int(n)) => u32::try_from(*n).is_ok(),
            _ => false,
        };
        if unix_ms {
            self.err(span, "a time (`unix_ms`) cannot have a default");
        } else if !fits {
            self.err(span, "the default does not fit the parameter's type");
        }
    }

    // ---- types -----------------------------------------------------------

    fn not_in_table(&mut self, ty: &syn::Type) {
        self.err(ty.span(), format!("`{}` is not in the type table (docs/phase-1-api.md, \"The type table\")", show(ty)));
    }

    /// A field's type, or the owned type of a parameter or return.
    fn owned_ty(&mut self, ty: &syn::Type) -> Option<Ty> {
        match ty {
            syn::Type::Paren(p) => self.owned_ty(&p.elem),
            syn::Type::Group(g) => self.owned_ty(&g.elem),
            syn::Type::Path(p) if p.qself.is_none() => {
                let seg = p.path.segments.last()?;
                let name = seg.ident.to_string();
                let Some(args) = type_args(seg) else {
                    self.not_in_table(ty);
                    return None;
                };
                match (name.as_str(), args.as_slice()) {
                    ("bool", []) => Some(Ty::Bool),
                    ("u32", []) => Some(Ty::U32),
                    ("u64", []) => Some(Ty::U64),
                    ("String", []) => Some(Ty::String),
                    ("Option", [inner]) => {
                        let t = self.owned_ty(inner)?;
                        if matches!(t, Ty::Option(_)) {
                            self.err(ty.span(), "Option<Option<…>> is not in the type table: say what null means some other way");
                            return None;
                        }
                        Some(Ty::Option(Box::new(t)))
                    }
                    ("Vec", [inner]) => Some(Ty::Vec(Box::new(self.owned_ty(inner)?))),
                    (n, []) if ID_TYPES.contains(&n) => Some(Ty::Id(name)),
                    (n, []) if NOT_IN_TABLE.contains(&n) => {
                        self.not_in_table(ty);
                        None
                    }
                    (_, []) => {
                        let loc = self.loc(seg.ident.span());
                        self.uses.push((name.clone(), loc));
                        Some(Ty::Named(name))
                    }
                    _ => {
                        self.not_in_table(ty);
                        None
                    }
                }
            }
            _ => {
                self.not_in_table(ty);
                None
            }
        }
    }

    /// A parameter's type in the params struct, and how it is passed.
    fn param_ty(&mut self, ty: &syn::Type) -> Option<(Ty, Pass)> {
        match ty {
            syn::Type::Reference(r) => {
                if r.mutability.is_some() {
                    self.err(ty.span(), "a parameter cannot be `&mut`");
                    return None;
                }
                Some((self.borrowed(&r.elem)?.0, Pass::Ref))
            }
            syn::Type::Path(p)
                if p.qself.is_none()
                    && let Some(seg) = p.path.segments.last()
                    && seg.ident == "Option"
                    && let Some(args) = type_args(seg)
                    && let [syn::Type::Reference(r)] = args.as_slice() =>
            {
                if r.mutability.is_some() {
                    self.err(ty.span(), "a parameter cannot be `&mut`");
                    return None;
                }
                let (inner, deref) = self.borrowed(&r.elem)?;
                Some((Ty::Option(Box::new(inner)), if deref { Pass::OptionDeref } else { Pass::OptionRef }))
            }
            _ => Some((self.owned_ty(ty)?, Pass::Value)),
        }
    }

    /// What a `&` parameter borrows: its owned type, and whether it is a
    /// `str` or a slice (passed with `as_deref` inside an Option).
    fn borrowed(&mut self, elem: &syn::Type) -> Option<(Ty, bool)> {
        match elem {
            syn::Type::Path(p) if p.qself.is_none() && p.path.is_ident("str") => Some((Ty::String, true)),
            syn::Type::Slice(s) => Some((Ty::Vec(Box::new(self.owned_ty(&s.elem)?)), true)),
            other => {
                let t = self.owned_ty(other)?;
                if matches!(t, Ty::Option(_)) {
                    self.err(other.span(), "borrow inside the Option: write Option<&T>");
                    return None;
                }
                Some((t, false))
            }
        }
    }

    /// A method's return: whether it is a `Result`, and what it carries.
    fn return_ty(&mut self, ty: &syn::Type) -> Option<(bool, Ty)> {
        if let syn::Type::Path(p) = ty
            && p.qself.is_none()
            && let Some(seg) = p.path.segments.last()
            && seg.ident == "Result"
        {
            return match type_args(seg).as_deref() {
                Some([inner]) => Some((true, self.plain_return(inner)?)),
                _ => {
                    self.err(ty.span(), "an API method returns post_core::Result<T>, not Result<T, E>");
                    None
                }
            };
        }
        Some((false, self.plain_return(ty)?))
    }

    fn plain_return(&mut self, ty: &syn::Type) -> Option<Ty> {
        if let syn::Type::Tuple(t) = ty
            && t.elems.is_empty()
        {
            return Some(Ty::Unit);
        }
        self.owned_ty(ty)
    }

    // ---- whole-API checks --------------------------------------------------

    fn check_uses(&mut self, model: &ApiModel) {
        let known: HashSet<&str> = model.types.iter().map(|t| t.name.as_str()).collect();
        for (name, loc) in std::mem::take(&mut self.uses) {
            if !known.contains(name.as_str()) {
                self.err_at(&loc, format!("`{name}` is used by the API but is neither in the type table nor marked #[api]"));
            }
        }
    }

    fn check_names(&mut self, model: &ApiModel) {
        let mut types: HashMap<String, Loc> = HashMap::new();
        for t in &model.types {
            if names::DART_TAKEN_TYPES.contains(&t.name.as_str()) {
                self.err_at(&t.loc, format!("`{}` is a name the Dart client already uses: rename the type", t.name));
            }
            self.claim(&mut types, &t.name, &t.loc);
            match &t.kind {
                TypeKind::Struct(fields) => self.check_fields(fields, &t.name),
                TypeKind::TaggedEnum(variants) => {
                    for v in variants {
                        self.claim(&mut types, &names::dart_variant_class(&v.name, &t.name), &v.loc);
                        self.check_fields(&v.fields, &format!("{}::{}", t.name, v.name));
                    }
                }
                TypeKind::UnitEnum(variants) => {
                    for v in variants {
                        let d = names::lower_first(&v.name);
                        if names::DART_RESERVED.contains(&d.as_str())
                            || names::DART_ENUM_MEMBERS.contains(&d.as_str())
                            || names::DART_OBJECT_MEMBERS.contains(&d.as_str())
                        {
                            self.err_at(&v.loc, format!("`{}::{}` becomes `{d}` in Dart, which it cannot use: rename it", t.name, v.name));
                        }
                    }
                }
            }
        }
        let mut methods: HashMap<String, Loc> = HashMap::new();
        for m in &model.methods {
            self.claim(&mut methods, &m.name, &m.loc);
            let d = names::lower_camel(&m.name);
            if names::DART_RESERVED.contains(&d.as_str())
                || names::DART_OBJECT_MEMBERS.contains(&d.as_str())
                || names::DART_API_MEMBERS.contains(&d.as_str())
            {
                self.err_at(&m.loc, format!("method `{}` becomes `{d}` in Dart, which it cannot use: rename it", m.name));
            }
            let mut params: HashMap<String, Loc> = HashMap::new();
            for p in &m.params {
                let d = names::dart_member(&p.name, p.unix_ms);
                if names::DART_RESERVED.contains(&d.as_str()) {
                    self.err_at(
                        &p.loc,
                        format!("parameter `{}` of `{}` becomes `{d}` in Dart, which it cannot use: rename it", p.name, m.name),
                    );
                }
                if let Some(prev) = params.insert(d.clone(), p.loc.clone()) {
                    self.err_at(&p.loc, format!("parameter `{}` of `{}` becomes `{d}` in Dart, as does the one at {prev}", p.name, m.name));
                }
            }
        }
    }

    fn check_fields(&mut self, fields: &[Field], owner: &str) {
        let mut seen: HashMap<String, Loc> = HashMap::new();
        for f in fields {
            let d = names::dart_member(&f.name, f.unix_ms);
            if names::DART_RESERVED.contains(&d.as_str())
                || names::DART_OBJECT_MEMBERS.contains(&d.as_str())
                || names::DART_GENERATED_MEMBERS.contains(&d.as_str())
            {
                self.err_at(&f.loc, format!("field `{}` of `{owner}` becomes `{d}` in Dart, which it cannot use: rename it", f.name));
            }
            if let Some(prev) = seen.insert(d.clone(), f.loc.clone()) {
                self.err_at(&f.loc, format!("field `{}` of `{owner}` becomes `{d}` in Dart, as does the one at {prev}", f.name));
            }
        }
    }

    /// Record `name` as declared at `loc`; report it if it already was.
    fn claim(&mut self, seen: &mut HashMap<String, Loc>, name: &str, loc: &Loc) {
        if let Some(prev) = seen.insert(name.to_owned(), loc.clone()) {
            self.err_at(loc, format!("`{name}` is declared twice in the API (also at {prev})"));
        }
    }
}
