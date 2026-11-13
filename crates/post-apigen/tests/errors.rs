//! Each error of docs/phase-1-api.md, "Generator errors", from a small
//! source: the generator names the file, line and column, and never guesses.

use post_apigen::Source;

/// The messages for `src`, read as `x.rs` at the crate root.
fn errors(src: &str) -> Vec<String> {
    match post_apigen::parse(&[Source { path: "x.rs", module: "", text: src }]) {
        Ok(_) => Vec::new(),
        Err(e) => e.0.iter().map(ToString::to_string).collect(),
    }
}

/// Exactly one error, containing `want`.
#[track_caller]
fn one(src: &str, want: &str) {
    let got = errors(src);
    assert!(got.len() == 1 && got[0].contains(want), "wanted one error containing {want:?}, got {got:#?}");
}

/// A method in `impl Core`, around `body`.
fn core(body: &str) -> String {
    format!("impl Core {{\n{body}\n}}")
}

#[test]
fn a_valid_source_has_no_errors() {
    let src = r#"
        /// A thing.
        #[api]
        pub struct Thing {
            /// Its name.
            pub name: String,
        }
        impl Core {
            /// The thing.
            #[api(needs = local)]
            pub fn thing(&self) -> Result<Thing> { todo!() }
        }
    "#;
    assert_eq!(errors(src), Vec::<String>::new());
}

#[test]
fn errors_name_the_file_line_and_column() {
    let src = "/// A.\n#[api]\npub struct A {\n    /// N.\n    pub n: i64,\n}\n";
    assert_eq!(errors(src), ["x.rs:5:12: `i64` is not in the type table (docs/phase-1-api.md, \"The type table\")"]);
}

#[test]
fn every_problem_is_reported_not_just_the_first() {
    let src = "#[api]\npub struct A {\n    pub n: i64,\n    pub m: bool,\n}\n";
    // A's doc, n's doc and type, m's doc.
    assert_eq!(errors(src).len(), 4, "{:#?}", errors(src));
}

#[test]
fn an_unknown_key() {
    one("/// A.\n#[api(colour)]\npub struct A {}", "unknown key `colour`");
    one(&core("/// M.\n#[api(needs = local, cached)]\npub fn m(&self) {}"), "unknown key `cached`");
}

#[test]
fn a_key_where_it_does_not_apply() {
    one("/// A.\n#[api(unix_ms)]\npub struct A {}", "`unix_ms` does not apply to a type");
    one("/// A.\n#[api]\npub struct A {\n/// N.\n#[api(needs = local)]\npub n: bool }", "`needs` does not apply to a field");
    one("/// A.\n#[api]\npub enum A {\n/// V.\n#[api(default)]\nV }", "`default` does not apply to a variant");
    one("/// A.\n#[api]\npub struct A {\n/// N.\n#[api(default = 3)]\npub n: u32 }", "`default = …` does not apply to a field");
    one(&core("/// M.\n#[api(needs = local, unix_ms)]\npub fn m(&self) {}"), "`unix_ms` does not apply to a method");
}

#[test]
fn a_method_that_is_not_pub() {
    one(&core("/// M.\n#[api(needs = local)]\nfn m(&self) {}"), "`m` must be pub");
}

#[test]
fn a_generic_method() {
    one(&core("/// M.\n#[api(needs = local)]\npub fn m<T>(&self) {}"), "`m` is generic");
}

#[test]
fn a_method_not_in_an_inherent_impl_core() {
    one("/// M.\n#[api(needs = local)]\npub fn m() {}", "#[api] goes on a struct, an enum, or a method in `impl Core`");
    one("impl Other {\n/// M.\n#[api(needs = local)]\npub fn m(&self) {}\n}", "must be in an inherent `impl Core` block");
    one("impl Trait for Core {\n/// M.\n#[api(needs = local)]\nfn m(&self) {}\n}", "must be in an inherent `impl Core` block");
}

#[test]
fn a_method_without_self() {
    one(&core("/// M.\n#[api(needs = local)]\npub fn m() {}"), "`m` must take `&self`");
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&mut self) {}"), "`m` must take `&self`");
}

#[test]
fn a_missing_or_unknown_needs() {
    one(&core("/// M.\n#[api]\npub fn m(&self) {}"), "`m` needs `#[api(needs = local | connection | registration)]`");
    one(&core("/// M.\n#[api(needs = online)]\npub fn m(&self) {}"), "unknown `needs = online`");
}

#[test]
fn a_type_outside_the_table() {
    one("/// A.\n#[api]\npub struct A {\n/// N.\npub n: f64 }", "`f64` is not in the type table");
    one("/// A.\n#[api]\npub struct A {\n/// N.\npub n: HashMap<String, u32> }", "`HashMap<String, u32>` is not in the type table");
    one("/// A.\n#[api]\npub struct A {\n/// N.\npub n: Option<Option<u32>> }", "Option<Option<…>> is not in the type table");
    one("/// A.\n#[api]\npub struct A {\n/// N.\npub n: &'static str }", "is not in the type table");
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&self,\n/// X.\nx: &mut String) {}"), "a parameter cannot be `&mut`");
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&self) -> Result<u32, MyError> { todo!() }"), "returns post_core::Result<T>");
}

#[test]
fn a_type_used_but_not_marked() {
    one(
        "/// A.\n#[api]\npub struct A {\n/// N.\npub n: Thing }",
        "`Thing` is used by the API but is neither in the type table nor marked #[api]",
    );
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&self) -> Result<Vec<Thing>> { todo!() }"), "`Thing` is used by the API");
}

#[test]
fn a_type_declared_later_is_found() {
    let src = "/// A.\n#[api]\npub struct A {\n/// B.\npub b: B }\n/// B.\n#[api]\npub enum B {\n/// X.\nX }";
    assert_eq!(errors(src), Vec::<String>::new());
}

#[test]
fn a_missing_doc_comment() {
    one("#[api]\npub struct A {}", "`A` needs a doc comment");
    one("/// A.\n#[api]\npub struct A {\npub n: bool }", "field `n` of `A` needs a doc comment");
    one("/// A.\n#[api]\npub enum A {\nV }", "variant `A::V` needs a doc comment");
    one(&core("#[api(needs = local)]\npub fn m(&self) {}"), "method `m` needs a doc comment");
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&self, x: bool) {}"), "parameter `x` of `m` needs a doc comment");
}

#[test]
fn shapes_that_are_not_allowed() {
    one("/// A.\n#[api]\npub struct A(u32);", "`A` is a tuple or unit struct");
    one("/// A.\n#[api]\npub struct A;", "`A` is a tuple or unit struct");
    one("/// A.\n#[api]\npub struct A {\n/// N.\nn: bool }", "field `n` of `A` must be pub");
    one("/// A.\n#[api]\npub enum A {\n/// V.\nV(u32) }", "`A::V` is a tuple variant");
    one("/// A.\n#[api]\npub struct A<T> {\n/// N.\npub n: bool }", "`A` is generic");
}

#[test]
fn misplaced_annotations_and_defaults() {
    one("/// A.\n#[api]\npub struct A {\n/// N.\n#[api(unix_ms)]\npub n: String }", "`unix_ms` goes on a u64 or an Option<u64>");
    one("/// A.\n#[api]\npub struct A {\n/// N.\n#[api(default)]\npub n: Option<bool> }", "already optional");
    one(
        "/// A.\n#[api]\npub struct A {\n/// N.\n#[api(default)]\npub n: B }\n/// B.\n#[api]\npub enum B {\n/// X.\nX }",
        "`default` goes on a bool",
    );
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&self,\n/// X.\n#[api(default = \"s\")]\nx: u32) {}"), "the default does not fit");
    one(
        &core("/// M.\n#[api(needs = local)]\npub fn m(&self,\n/// X.\n#[api(default = 5000000000)]\nx: u32) {}"),
        "the default does not fit",
    );
    one(&core("/// M.\n#[api(needs = local)]\npub fn m(&self,\n/// X.\n#[api(default, default = 1)]\nx: u32) {}"), "more than one default");
}

#[test]
fn names_dart_cannot_use() {
    one("/// A.\n#[api]\npub struct A {\n/// N.\npub new: bool }", "field `new` of `A` becomes `new` in Dart");
    one("/// A.\n#[api]\npub struct A {\n/// N.\npub hash_code: bool }", "becomes `hashCode` in Dart");
    one("/// A.\n#[api]\npub enum A {\n/// V.\nValues }", "`A::Values` becomes `values` in Dart");
    one(&core("/// M.\n#[api(needs = local)]\npub fn switch(&self) {}"), "method `switch` becomes `switch` in Dart");
    one(&core("/// M.\n#[api(needs = local)]\npub fn events(&self) {}"), "method `events` becomes `events` in Dart");
    one(
        &core("/// M.\n#[api(needs = local)]\npub fn m(&self,\n/// X.\nrequired: bool) {}"),
        "parameter `required` of `m` becomes `required`",
    );
    one("/// A.\n#[api]\npub struct List {}", "`List` is a name the Dart client already uses");
    one(
        "/// A.\n#[api]\npub struct A {\n/// N.\npub at: u64,\n/// M.\n#[api(unix_ms)]\npub at_ms: u64 }",
        "becomes `at` in Dart, as does the one at x.rs",
    );
}

#[test]
fn names_declared_twice() {
    let a = "/// A.\n#[api]\npub struct A {}";
    let got = match post_apigen::parse(&[Source { path: "a.rs", module: "", text: a }, Source { path: "b.rs", module: "m", text: a }]) {
        Ok(_) => Vec::new(),
        Err(e) => e.0.iter().map(ToString::to_string).collect(),
    };
    assert_eq!(got, ["b.rs:3:12: `A` is declared twice in the API (also at a.rs:3:12)"]);
    one("/// E.\n#[api]\npub enum E {\n/// V.\nV { /// X.\nx: bool } }\n/// VE.\n#[api]\npub struct VE {}", "`VE` is declared twice");
}

#[test]
fn a_raw_identifier() {
    one("/// A.\n#[api]\npub struct A {\n/// T.\npub r#type: bool }", "`r#type` is a raw identifier");
}

#[test]
fn a_file_that_does_not_parse() {
    let got = errors("pub struct {");
    assert!(got.len() == 1 && got[0].starts_with("x.rs:1:"), "{got:?}");
}
