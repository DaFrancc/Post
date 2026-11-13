//! The Rust outputs: `api.generated.rs` (params structs, the method table and
//! `dispatch`, included into post-api) and the fixtures round-trip test.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::Config;
use crate::model::{ApiModel, Lit, Method, Needs, ParamDefault, Pass, Ty, summary};
use crate::names;

/// Print `tokens` as a Rust file under the generated header.
fn print(tokens: TokenStream, cfg: &Config) -> String {
    let file: syn::File = syn::parse2(tokens).expect("the generator writes valid Rust");
    format!("// {}\n\n{}", cfg.header, prettyplease::unparse(&file))
}

/// One `#[doc]` per line, so they print as `///` lines.
fn doc_attrs(docs: &str) -> TokenStream {
    let lines = docs.split('\n').map(|l| if l.is_empty() { String::new() } else { format!(" {l}") });
    quote!(#(#[doc = #lines])*)
}

/// A type as the params structs and the fixtures test name it.
fn rust_ty(ty: &Ty, model: &ApiModel, cfg: &Config) -> TokenStream {
    let krate = format_ident!("{}", cfg.core_crate);
    match ty {
        Ty::Unit => quote!(()),
        Ty::Bool => quote!(bool),
        Ty::U32 => quote!(u32),
        Ty::U64 => quote!(u64),
        Ty::String => quote!(String),
        Ty::Id(n) => {
            let n = format_ident!("{n}");
            quote!(::#krate::#n)
        }
        Ty::Named(n) => {
            let path = model.ty(n).map_or(n.as_str(), |t| t.path.as_str());
            let segs = path.split("::").map(|s| format_ident!("{s}"));
            quote!(::#krate::#(#segs)::*)
        }
        Ty::Option(t) => {
            let t = rust_ty(t, model, cfg);
            quote!(Option<#t>)
        }
        Ty::Vec(t) => {
            let t = rust_ty(t, model, cfg);
            quote!(Vec<#t>)
        }
    }
}

fn params_name(m: &Method) -> syn::Ident {
    format_ident!("{}Params", names::upper_camel(&m.name))
}

fn params_struct(m: &Method, model: &ApiModel, cfg: &Config) -> TokenStream {
    if m.params.is_empty() {
        return quote!();
    }
    let name = params_name(m);
    let doc = format!(" Parameters of `{}`.", m.name);
    let mut default_fns = Vec::new();
    let mut fields = Vec::new();
    for p in &m.params {
        let docs = doc_attrs(&p.docs);
        let field = format_ident!("{}", p.name);
        let ty = rust_ty(&p.ty, model, cfg);
        let serde = match &p.default {
            ParamDefault::None => quote!(),
            ParamDefault::Type => quote!(#[serde(default)]),
            ParamDefault::Value(lit) => {
                let f = format!("default_{}_{}", m.name, p.name);
                let fi = format_ident!("{f}");
                let value = match lit {
                    Lit::Bool(b) => quote!(#b),
                    Lit::Int(n) => {
                        let n = Literal::u64_unsuffixed(*n);
                        quote!(#n)
                    }
                    Lit::Str(s) => quote!(#s.to_owned()),
                };
                default_fns.push(quote!(fn #fi() -> #ty { #value }));
                quote!(#[serde(default = #f)])
            }
        };
        fields.push(quote!(#docs #serde pub #field: #ty));
    }
    quote! {
        #[doc = #doc]
        #[derive(Clone, Debug, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct #name { #(#fields),* }
        #(#default_fns)*
    }
}

fn dispatch_fn(m: &Method, cfg: &Config) -> TokenStream {
    let krate = format_ident!("{}", cfg.core_crate);
    let f = format_ident!("dispatch_{}", m.name);
    let method = format_ident!("{}", m.name);
    let decode = if m.params.is_empty() {
        quote!(no_params(p)?;)
    } else {
        let ps = params_name(m);
        quote!(let p: #ps = params(p)?;)
    };
    let args = m.params.iter().map(|p| {
        let x = format_ident!("{}", p.name);
        match p.pass {
            Pass::Value => quote!(p.#x),
            Pass::Ref => quote!(&p.#x),
            Pass::OptionDeref => quote!(p.#x.as_deref()),
            Pass::OptionRef => quote!(p.#x.as_ref()),
        }
    });
    let mut call = quote!(core.#method(#(#args),*));
    if m.is_async {
        call = quote!(#call.await);
    }
    if m.fallible {
        call = quote!(#call?);
    }
    let body = if m.ret == Ty::Unit { quote!(#call; Ok(::serde_json::Value::Null)) } else { quote!(result(#call)) };
    quote! {
        async fn #f(core: &::#krate::Core, p: ::serde_json::Value) -> Result<::serde_json::Value, CallError> {
            #decode
            #body
        }
    }
}

/// `api.generated.rs`: params structs, the method table, `dispatch`, and
/// the fixtures test (here rather than in `tests/`, where rustfmt would
/// reformat it; an `include!`d file it leaves alone).
pub fn dispatch(model: &ApiModel, cfg: &Config) -> String {
    let krate = format_ident!("{}", cfg.core_crate);
    let structs = model.methods.iter().map(|m| params_struct(m, model, cfg));
    let fns = model.methods.iter().map(|m| dispatch_fn(m, cfg));
    let infos = model.methods.iter().map(|m| {
        let name = &m.name;
        let summary = summary(&m.docs);
        let needs = format_ident!(
            "{}",
            match m.needs {
                Needs::Local => "Local",
                Needs::Connection => "Connection",
                Needs::Registration => "Registration",
            }
        );
        quote!(MethodInfo { name: #name, summary: #summary, needs: Needs::#needs })
    });
    let arms = model.methods.iter().map(|m| {
        let name = &m.name;
        let f = format_ident!("dispatch_{}", m.name);
        quote!(#name => #f(core, p).await,)
    });
    let fixtures = fixtures_test(model, cfg);
    print(
        quote! {
            #(#structs)*
            #(#fns)*

            /// Every method, in the order post-core declares them.
            pub const METHODS: &[MethodInfo] = &[#(#infos),*];

            /// Run one method, with no connection handling.
            pub async fn dispatch(core: &::#krate::Core, name: &str, p: ::serde_json::Value) -> Result<::serde_json::Value, CallError> {
                match name {
                    #(#arms)*
                    _ => Err(CallError::UnknownMethod(name.to_owned())),
                }
            }

            #fixtures
        },
        cfg,
    )
}

/// The fixtures test: each API type reads its samples and writes them back
/// unchanged, so the generator's idea of the JSON form is serde's.
fn fixtures_test(model: &ApiModel, cfg: &Config) -> TokenStream {
    let checks = model.types.iter().map(|t| {
        let ty = rust_ty(&Ty::Named(t.name.clone()), model, cfg);
        let name = &t.name;
        quote!(round_trip::<#ty>(&all, #name);)
    });
    let include = &cfg.fixtures_from_rust;
    quote! {
        /// Each API type reads its fixtures (one sample per type, per variant for enums) and writes them back unchanged:
        /// the generator's idea of the JSON form is serde's. The Dart client is held to the same fixtures.
        #[cfg(test)]
        mod fixtures {
            fn round_trip<T: ::serde::Serialize + ::serde::de::DeserializeOwned>(all: &::serde_json::Value, name: &str) {
                let samples = all[name].as_array().unwrap_or_else(|| panic!("no fixtures for {name}"));
                for v in samples {
                    let t: T = ::serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("{name} does not read {v}: {e}"));
                    let back = ::serde_json::to_value(&t).unwrap_or_else(|e| panic!("{name} does not write: {e}"));
                    assert_eq!(back, *v, "{name} reads {v} but writes {back}");
                }
            }

            #[test]
            fn every_api_type_reads_and_writes_its_fixtures() {
                let all: ::serde_json::Value = ::serde_json::from_str(include_str!(#include)).expect("the fixtures are JSON");
                #(#checks)*
            }
        }
    }
}
