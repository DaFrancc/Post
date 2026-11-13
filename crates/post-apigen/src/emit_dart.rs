//! The Dart outputs: `post_api.dart` (a class per API type, the `PostApi`
//! interface and `RpcPostApi` over a transport) and its fixtures test. The
//! client imports nothing from Flutter, so plain Dart can test it.

use crate::Config;
use crate::model::{ApiModel, Field, Lit, Method, ParamDefault, Ty, TypeDef, TypeKind, summary};
use crate::names::{dart_member, dart_variant_class, lower_camel, lower_first, serde_snake};

/// Lines of Dart, two spaces per indent level.
struct W {
    out: String,
}

impl W {
    fn line(&mut self, indent: usize, s: impl AsRef<str>) {
        let s = s.as_ref();
        if !s.is_empty() {
            self.out.push_str(&"  ".repeat(indent));
            self.out.push_str(s);
        }
        self.out.push('\n');
    }

    fn blank(&mut self) {
        self.out.push('\n');
    }

    fn docs(&mut self, indent: usize, docs: &str) {
        for l in docs.split('\n') {
            if l.is_empty() {
                self.line(indent, "///");
            } else {
                self.line(indent, format!("/// {l}"));
            }
        }
    }
}

fn header(w: &mut W, cfg: &Config) {
    w.line(0, format!("// {}", cfg.header));
    w.line(0, "// dart format off");
    w.line(0, "// ignore_for_file: type=lint");
}

/// A Dart string literal.
fn dart_string(v: &str) -> String {
    let escaped = v.replace('\\', "\\\\").replace('\'', "\\'").replace('$', "\\$").replace('\n', "\\n");
    format!("'{escaped}'")
}

struct Dart<'a> {
    model: &'a ApiModel,
    /// Some `==` compared lists, so the helpers are needed.
    uses_deep: bool,
}

impl Dart<'_> {
    fn is_unit_enum(&self, name: &str) -> bool {
        matches!(self.model.ty(name).map(|t| &t.kind), Some(TypeKind::UnitEnum(_)))
    }

    fn ty(&self, ty: &Ty, unix_ms: bool) -> String {
        match ty {
            Ty::Unit => "void".to_owned(),
            Ty::Bool => "bool".to_owned(),
            Ty::U32 | Ty::U64 if unix_ms => "DateTime".to_owned(),
            Ty::U32 | Ty::U64 => "int".to_owned(),
            Ty::String | Ty::Id(_) => "String".to_owned(),
            Ty::Named(n) => n.clone(),
            Ty::Option(t) => format!("{}?", self.ty(t, unix_ms)),
            Ty::Vec(t) => format!("List<{}>", self.ty(t, unix_ms)),
        }
    }

    /// Dart reading the JSON value `e` as `ty`.
    fn decode(&self, ty: &Ty, unix_ms: bool, e: &str, depth: usize) -> String {
        match ty {
            Ty::Unit => "null".to_owned(),
            Ty::Bool => format!("{e} as bool"),
            Ty::U32 | Ty::U64 if unix_ms => format!("DateTime.fromMillisecondsSinceEpoch({e} as int)"),
            Ty::U32 | Ty::U64 => format!("{e} as int"),
            Ty::String | Ty::Id(_) => format!("{e} as String"),
            Ty::Named(n) if self.is_unit_enum(n) => format!("{n}.fromJson({e})"),
            Ty::Named(n) => format!("{n}.fromJson({e} as Map<String, Object?>)"),
            Ty::Option(t) => match &**t {
                Ty::Bool | Ty::String | Ty::Id(_) => format!("{e} as {}?", self.ty(t, false)),
                Ty::U32 | Ty::U64 if !unix_ms => format!("{e} as int?"),
                _ => {
                    let v = format!("v{depth}");
                    format!("switch ({e}) {{ null => null, final {v} => {} }}", self.decode(t, unix_ms, &v, depth + 1))
                }
            },
            Ty::Vec(t) => {
                let v = format!("e{depth}");
                format!("({e} as List<Object?>).map(({v}) => {}).toList()", self.decode(t, unix_ms, &v, depth + 1))
            }
        }
    }

    /// Dart writing `e`, of type `ty`, as a JSON value.
    fn encode(&self, ty: &Ty, unix_ms: bool, e: &str, depth: usize) -> String {
        match ty {
            Ty::U32 | Ty::U64 if unix_ms => format!("{e}.millisecondsSinceEpoch"),
            Ty::Named(n) if self.is_unit_enum(n) => format!("{e}.json"),
            Ty::Named(_) => format!("{e}.toJson()"),
            Ty::Option(t) => {
                // Every conversion is `e.member…`: null-shorting does the rest.
                let inner = self.encode(t, unix_ms, e, depth);
                if inner == e { inner } else { format!("{e}?{}", &inner[e.len()..]) }
            }
            Ty::Vec(t) => {
                let v = format!("e{depth}");
                let inner = self.encode(t, unix_ms, &v, depth + 1);
                if inner == v { e.to_owned() } else { format!("{e}.map(({v}) => {inner}).toList()") }
            }
            _ => e.to_owned(),
        }
    }

    /// The Dart value of `Default::default()` for a type allowed `default`.
    fn type_default(&self, ty: &Ty) -> String {
        match ty {
            Ty::Bool => "false".to_owned(),
            Ty::String => "''".to_owned(),
            Ty::Vec(t) => format!("const <{}>[]", self.ty(t, false)),
            _ => "0".to_owned(),
        }
    }

    fn field_decode(&self, f: &Field) -> String {
        let key = format!("j['{}']", f.name);
        if f.default {
            format!("switch ({key}) {{ null => {}, final v0 => {} }}", self.type_default(&f.ty), self.decode(&f.ty, f.unix_ms, "v0", 1))
        } else {
            self.decode(&f.ty, f.unix_ms, &key, 0)
        }
    }

    /// A class for a struct, or for one variant of a tagged enum.
    fn class(&mut self, w: &mut W, cls: &str, docs: &str, fields: &[Field], tag: Option<(&str, &str)>) {
        w.docs(0, docs);
        match tag {
            Some((base, _)) => w.line(0, format!("final class {cls} extends {base} {{")),
            None => w.line(0, format!("final class {cls} {{")),
        }
        if fields.is_empty() {
            w.line(1, format!("const {cls}();"));
        } else {
            w.line(1, format!("const {cls}({{"));
            for f in fields {
                let name = dart_member(&f.name, f.unix_ms);
                if matches!(f.ty, Ty::Option(_)) {
                    w.line(2, format!("this.{name},"));
                } else if f.default {
                    w.line(2, format!("this.{name} = {},", self.type_default(&f.ty)));
                } else {
                    w.line(2, format!("required this.{name},"));
                }
            }
            w.line(1, "});");
        }
        w.blank();
        w.line(1, format!("/// Reads [{cls}] from its JSON form."));
        if fields.is_empty() {
            w.line(1, format!("factory {cls}.fromJson(Map<String, Object?> j) => const {cls}();"));
        } else {
            w.line(1, format!("factory {cls}.fromJson(Map<String, Object?> j) => {cls}("));
            for f in fields {
                w.line(2, format!("{}: {},", dart_member(&f.name, f.unix_ms), self.field_decode(f)));
            }
            w.line(1, ");");
        }
        for f in fields {
            w.blank();
            w.docs(1, &f.docs);
            w.line(1, format!("final {} {};", self.ty(&f.ty, f.unix_ms), dart_member(&f.name, f.unix_ms)));
        }
        w.blank();
        w.line(1, "/// Its JSON form.");
        if tag.is_some() {
            w.line(1, "@override");
        }
        w.line(1, "Map<String, Object?> toJson() => {");
        if let Some((_, json)) = tag {
            w.line(2, format!("'type': '{json}',"));
        }
        for f in fields {
            w.line(2, format!("'{}': {},", f.name, self.encode(&f.ty, f.unix_ms, &dart_member(&f.name, f.unix_ms), 0)));
        }
        w.line(1, "};");
        w.blank();
        w.line(1, "@override");
        let mut eq = format!("other is {cls}");
        for f in fields {
            let n = dart_member(&f.name, f.unix_ms);
            if f.ty.has_list() {
                self.uses_deep = true;
                eq.push_str(&format!(" && _deepEquals(other.{n}, {n})"));
            } else {
                eq.push_str(&format!(" && other.{n} == {n}"));
            }
        }
        w.line(1, format!("bool operator ==(Object other) => {eq};"));
        w.blank();
        w.line(1, "@override");
        if fields.is_empty() {
            w.line(1, "int get hashCode => runtimeType.hashCode;");
        } else {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| {
                    let n = dart_member(&f.name, f.unix_ms);
                    if f.ty.has_list() { format!("_deepHash({n})") } else { n }
                })
                .collect();
            w.line(1, format!("int get hashCode => Object.hashAll([{}]);", parts.join(", ")));
        }
        w.line(0, "}");
    }

    fn unit_enum(&self, w: &mut W, t: &TypeDef, variants: &[crate::model::Variant]) {
        w.docs(0, &t.docs);
        w.line(0, format!("enum {} {{", t.name));
        for (i, v) in variants.iter().enumerate() {
            w.docs(1, &v.docs);
            let end = if i + 1 < variants.len() { "," } else { ";" };
            w.line(1, format!("{}('{}'){end}", lower_first(&v.name), serde_snake(&v.name)));
        }
        w.blank();
        w.line(1, format!("const {}(this.json);", t.name));
        w.blank();
        w.line(1, "/// Its JSON form.");
        w.line(1, "final String json;");
        w.blank();
        w.line(1, format!("/// Reads [{}] from its JSON form.", t.name));
        w.line(
            1,
            format!(
                "static {0} fromJson(Object? v) => values.firstWhere((e) => e.json == v, orElse: () => throw FormatException('unknown {0}: $v'));",
                t.name
            ),
        );
        w.line(0, "}");
    }

    fn tagged_enum(&mut self, w: &mut W, t: &TypeDef, variants: &[crate::model::Variant]) {
        w.docs(0, &t.docs);
        w.line(0, format!("sealed class {} {{", t.name));
        w.line(1, format!("const {}();", t.name));
        w.blank();
        w.line(1, format!("/// Reads [{}] from its JSON form.", t.name));
        w.line(1, format!("factory {}.fromJson(Map<String, Object?> j) => switch (j['type']) {{", t.name));
        for v in variants {
            w.line(2, format!("'{}' => {}.fromJson(j),", serde_snake(&v.name), dart_variant_class(&v.name, &t.name)));
        }
        w.line(2, format!("final t => throw FormatException('unknown {}: $t'),", t.name));
        w.line(1, "};");
        w.blank();
        w.line(1, "/// Its JSON form.");
        w.line(1, "Map<String, Object?> toJson();");
        w.line(0, "}");
        for v in variants {
            w.blank();
            let json = serde_snake(&v.name);
            self.class(w, &dart_variant_class(&v.name, &t.name), &v.docs, &v.fields, Some((&t.name, &json)));
        }
    }

    /// `({required String device, int days = 7})`.
    fn signature(&self, m: &Method) -> String {
        if m.params.is_empty() {
            return "()".to_owned();
        }
        let params: Vec<String> = m
            .params
            .iter()
            .map(|p| {
                let ty = self.ty(&p.ty, p.unix_ms);
                let name = dart_member(&p.name, p.unix_ms);
                match &p.default {
                    ParamDefault::None if matches!(p.ty, Ty::Option(_)) => format!("{ty} {name}"),
                    ParamDefault::None => format!("required {ty} {name}"),
                    ParamDefault::Type => format!("{ty} {name} = {}", self.type_default(&p.ty)),
                    ParamDefault::Value(Lit::Bool(b)) => format!("{ty} {name} = {b}"),
                    ParamDefault::Value(Lit::Int(n)) => format!("{ty} {name} = {n}"),
                    ParamDefault::Value(Lit::Str(v)) => format!("{ty} {name} = {}", dart_string(v)),
                }
            })
            .collect();
        format!("({{{}}})", params.join(", "))
    }

    fn method_docs(&self, w: &mut W, m: &Method) {
        w.docs(1, &m.docs);
        if !m.params.is_empty() {
            w.line(1, "///");
            for p in &m.params {
                w.line(1, format!("/// - [{}]: {}", dart_member(&p.name, p.unix_ms), summary(&p.docs)));
            }
        }
    }

    fn api(&self, w: &mut W, has_events: bool) {
        w.line(0, "/// How the client reaches the core: one call at a time, and the event stream.");
        w.line(0, "abstract interface class PostTransport {");
        w.line(1, "/// Run [method] with [params] (JSON values); its result, a JSON value.");
        w.line(1, "Future<Object?> call(String method, Map<String, Object?> params);");
        w.blank();
        w.line(1, "/// Live events, each a JSON object.");
        w.line(1, "Stream<Map<String, Object?>> get events;");
        w.line(0, "}");
        w.blank();
        w.line(0, "/// The local API: what the app can ask of the core.");
        w.line(0, "abstract interface class PostApi {");
        if has_events {
            w.line(1, "/// Live events.");
            w.line(1, "Stream<Event> get events;");
        }
        for m in &self.model.methods {
            w.blank();
            self.method_docs(w, m);
            w.line(1, format!("Future<{}> {}{};", self.ty(&m.ret, false), lower_camel(&m.name), self.signature(m)));
        }
        w.line(0, "}");
        w.blank();
        w.line(0, "/// [PostApi] over a [PostTransport].");
        w.line(0, "final class RpcPostApi implements PostApi {");
        w.line(1, "/// Calls go through [transport].");
        w.line(1, "RpcPostApi(this.transport);");
        w.blank();
        w.line(1, "/// How calls reach the core.");
        w.line(1, "final PostTransport transport;");
        if has_events {
            w.blank();
            w.line(1, "@override");
            w.line(1, "Stream<Event> get events => transport.events.map(Event.fromJson);");
        }
        for m in &self.model.methods {
            w.blank();
            w.line(1, "@override");
            w.line(1, format!("Future<{}> {}{} async {{", self.ty(&m.ret, false), lower_camel(&m.name), self.signature(m)));
            let call = if m.params.is_empty() {
                format!("await transport.call('{}', const {{}});", m.name)
            } else {
                format!("await transport.call('{}', {{", m.name)
            };
            let start = if m.ret == Ty::Unit { call } else { format!("final r = {call}") };
            w.line(2, start);
            if !m.params.is_empty() {
                for p in &m.params {
                    w.line(3, format!("'{}': {},", p.name, self.encode(&p.ty, p.unix_ms, &dart_member(&p.name, p.unix_ms), 0)));
                }
                w.line(2, "});");
            }
            if m.ret != Ty::Unit {
                w.line(2, format!("return {};", self.decode(&m.ret, false, "r", 0)));
            }
            w.line(1, "}");
        }
        w.line(0, "}");
    }
}

/// `post_api.dart`.
pub fn client(model: &ApiModel, cfg: &Config) -> String {
    let mut w = W { out: String::new() };
    header(&mut w, cfg);
    w.line(0, "//");
    w.line(0, "// The local API (docs/phase-1-api.md): a class for each type the core's");
    w.line(0, "// #[api] marks, and [PostApi], every #[api] method on Core.");
    let mut d = Dart { model, uses_deep: false };
    for t in &model.types {
        w.blank();
        match &t.kind {
            TypeKind::Struct(fields) => d.class(&mut w, &t.name, &t.docs, fields, None),
            TypeKind::UnitEnum(vs) => d.unit_enum(&mut w, t, vs),
            TypeKind::TaggedEnum(vs) => d.tagged_enum(&mut w, t, vs),
        }
    }
    let has_events = matches!(model.ty("Event").map(|t| &t.kind), Some(TypeKind::TaggedEnum(_)));
    w.blank();
    d.api(&mut w, has_events);
    if d.uses_deep {
        w.blank();
        w.line(0, "bool _deepEquals(Object? a, Object? b) {");
        w.line(1, "if (a is List && b is List) {");
        w.line(2, "if (a.length != b.length) return false;");
        w.line(2, "for (var i = 0; i < a.length; i++) {");
        w.line(3, "if (!_deepEquals(a[i], b[i])) return false;");
        w.line(2, "}");
        w.line(2, "return true;");
        w.line(1, "}");
        w.line(1, "return a == b;");
        w.line(0, "}");
        w.blank();
        w.line(0, "int _deepHash(Object? v) => v is List ? Object.hashAll(v.map(_deepHash)) : v.hashCode;");
    }
    w.out
}

/// `api_fixtures_test.dart`.
pub fn fixtures_test(model: &ApiModel, cfg: &Config) -> String {
    let mut w = W { out: String::new() };
    header(&mut w, cfg);
    w.blank();
    w.line(0, "import 'dart:convert';");
    w.line(0, "import 'dart:io';");
    w.blank();
    w.line(0, format!("import '{}';", cfg.dart_test_import));
    w.line(0, format!("import 'package:{}/{}';", cfg.dart_package, cfg.dart_client));
    w.blank();
    w.line(0, "/// Each API type reads its fixtures and writes them back unchanged: the");
    w.line(0, "/// Dart classes agree with serde, which the Rust fixtures test checks.");
    w.line(0, "void main() {");
    w.line(1, format!("final all = jsonDecode(File('{}').readAsStringSync()) as Map<String, Object?>;", cfg.fixtures_from_dart_test));
    w.line(1, "List<Object?> samples(String name) => all[name] as List<Object?>;");
    for t in &model.types {
        w.blank();
        w.line(1, format!("test('{}', () {{", t.name));
        w.line(2, format!("for (final v in samples('{}')) {{", t.name));
        match &t.kind {
            TypeKind::UnitEnum(_) => w.line(3, format!("expect({}.fromJson(v).json, v);", t.name)),
            _ => w.line(3, format!("expect({}.fromJson(v as Map<String, Object?>).toJson(), v);", t.name)),
        }
        w.line(2, "}");
        w.line(1, "});");
    }
    w.line(0, "}");
    w.out
}
