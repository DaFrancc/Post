//! The `#[api]` marker read by `post-apigen` (docs/phase-1-api.md). It gives
//! API types their JSON form and removes itself; the generator gives the
//! annotations their meaning and checks them. This macro validates nothing.

use proc_macro::TokenStream;
use quote::{ToTokens, quote};
use syn::visit_mut::{self, VisitMut};

fn is_api(a: &syn::Attribute) -> bool {
    a.path().is_ident("api")
}

/// Whether an `#[api(...)]` among `attrs` has the bare key `default`.
fn has_default(attrs: &[syn::Attribute]) -> bool {
    let mut found = false;
    for a in attrs.iter().filter(|a| is_api(a)) {
        // Errors are the generator's to report; here a malformed attribute
        // just has no `default`.
        let _ = a.parse_nested_meta(|m| {
            if m.input.peek(syn::Token![=]) {
                let _: syn::Expr = m.value()?.parse()?;
            } else if m.path.is_ident("default") {
                found = true;
            }
            Ok(())
        });
    }
    found
}

/// Removes the annotations the compiler must not see, and gives fields marked
/// `default` their serde default.
struct Strip;

impl VisitMut for Strip {
    fn visit_field_mut(&mut self, f: &mut syn::Field) {
        if has_default(&f.attrs) {
            f.attrs.push(syn::parse_quote!(#[serde(default)]));
        }
        f.attrs.retain(|a| !is_api(a));
        visit_mut::visit_field_mut(self, f);
    }

    fn visit_variant_mut(&mut self, v: &mut syn::Variant) {
        v.attrs.retain(|a| !is_api(a));
        visit_mut::visit_variant_mut(self, v);
    }

    fn visit_fn_arg_mut(&mut self, a: &mut syn::FnArg) {
        // Doc comments are not allowed on parameters once macros have run;
        // the generator has read them from the source.
        if let syn::FnArg::Typed(t) = a {
            t.attrs.retain(|a| !is_api(a) && !a.path().is_ident("doc"));
        }
    }
}

/// Marks a `Core` method or a type as part of the local API. On a type it
/// adds the serde derives and fixes the JSON form; on a method it only
/// removes the parameter annotations. See `docs/phase-1-api.md`.
#[proc_macro_attribute]
pub fn api(_args: TokenStream, item: TokenStream) -> TokenStream {
    if let Ok(mut f) = syn::parse::<syn::ImplItemFn>(item.clone()) {
        Strip.visit_impl_item_fn_mut(&mut f);
        return f.into_token_stream().into();
    }
    match syn::parse::<syn::Item>(item) {
        Ok(syn::Item::Struct(mut s)) => {
            Strip.visit_item_struct_mut(&mut s);
            quote!(#[derive(::serde::Serialize, ::serde::Deserialize)] #[serde(deny_unknown_fields)] #s).into()
        }
        Ok(syn::Item::Enum(mut e)) => {
            Strip.visit_item_enum_mut(&mut e);
            if e.variants.iter().all(|v| matches!(v.fields, syn::Fields::Unit)) {
                quote!(#[derive(::serde::Serialize, ::serde::Deserialize)] #[serde(rename_all = "snake_case")] #e).into()
            } else {
                quote!(
                    #[derive(::serde::Serialize, ::serde::Deserialize)]
                    #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
                    #e
                )
                .into()
            }
        }
        Ok(other) => syn::Error::new_spanned(other, "#[api] goes on a struct, an enum or a method").to_compile_error().into(),
        Err(e) => e.to_compile_error().into(),
    }
}
