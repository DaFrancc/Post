# Prototype: generating the local API glue (`post-apigen`)

A design for whoever implements the prototype. Read it top to bottom once; the
sections after "Decisions" are the specification.

## 1. What this is for

Adding one field or one method to Post's client API currently means editing the
same information by hand in several crates:

| Layer | File | What is hand-written per method / type |
|---|---|---|
| Core | `crates/post-core/src/lib.rs` | the method on `Core`, the type |
| JSON-RPC | `crates/post-api/src/lib.rs` | a params struct, a result struct, a mapper, a row in `methods!`, an arm in `call` |
| App bridge | `crates/post-ffi/src/api/client.rs` | a struct (`…Info`), a method on `Client`, a mapper |
| CLI | `crates/post-cli/src/main.rs` | a clap variant, a `json!` mapping, a `print_human` arm |

The profile bio (commit `76fc181`) touched roughly 30 places before any UI.
`post-api` and `post-ffi` do not share types, and their names have drifted.

The prototype proves one idea on a small slice: **declare a method or type once
in `post-core` with an `#[api]` annotation, and generate the `post-api` and
`post-ffi` code for it.** The model is `tools/reflectgen` in the Assisi repo:
annotations on the real declarations, a type table, hard errors for anything
unsupported, readable generated files, golden tests.

The prototype succeeds if, for the slice below:

1. the hand-written `post-api` and `post-ffi` code for it is deleted and replaced
   by generated code;
2. nothing observable changes: same JSON-RPC methods, parameters and results,
   same Dart API, all existing tests pass;
3. adding a field to a slice type afterwards needs hand edits in `post-core`
   only (section 9, "Payoff check").

It is a prototype: it converts 9 of 75 methods. Converting the rest is a later
step, decided after seeing the result.

## 2. The slice

Nine methods on `Core` and three types, chosen to cover each shape the
generator must handle.

| Core method | Shape it exercises |
|---|---|
| `devices() -> Result<Vec<Device>>` | no parameters, list result |
| `rename_device(device: &str, name: &str) -> Result<()>` | string parameters, no result |
| `remove_device(device: &str) -> Result<()>` | |
| `refresh_keys() -> Result<()>` | no parameters, no result |
| `link_inspect(code: &str) -> Result<LinkRequest>` | struct result |
| `link_approve(code: &str) -> Result<LinkRequest>` | |
| `create_invite(bot: bool, days: u32) -> Result<Invite>` | parameter defaults, a renamed type, a time field |
| `set_admin(conversation: ConversationId, username: &str, admin: bool) -> Result<()>` | an ID parameter |
| `leave_group(conversation: ConversationId) -> Result<()>` | |

| Core type | JSON-RPC name today | Bridge name today |
|---|---|---|
| `Device` | `Device` | `DeviceInfo` |
| `LinkRequest` | `LinkRequest` | `LinkRequestInfo` |
| `Invite` | `InviteCode` | `InviteInfo` |

All nine have the same method name in core, `post-api` and `post-ffi` today.

## 3. Decisions (already made; do not reopen)

1. **The source of truth is `post-core`.** Annotations go on the real `Core`
   methods and the real structs, not in a schema file.
2. **A separate generator writes real `.rs` files**, which are committed. It
   is not a proc macro that expands inside `post-api` / `post-ffi`. Generated
   files are readable, show up in diffs and can be golden-tested.
3. **The `#[api]` attribute itself is a no-op proc macro.** It exists so the
   annotations compile; it only strips them. All logic is in the generator.
4. **Generated files are pulled in with `include!`**, not as modules:
   - `crates/post-api/src/api.generated.rs`, included from `lib.rs`
   - `crates/post-ffi/src/api/client.generated.rs`, included from `client.rs`

   Reasons: the generated bridge types keep the Rust path
   `crate::api::client::…`, so the committed `frb_generated.rs` and all Dart
   code stay valid without re-running `flutter_rust_bridge_codegen`; generated
   code can use the including file's private helpers and `Client`'s private
   `core` field; a hand-written duplicate left behind is a compile error rather
   than a silent shadow; rustfmt never touches the files.
5. **CLI commands are not generated.** They are not a mechanical projection of
   the API (`devices remove` asks for confirmation, `group admin --revoke`
   inverts `admin`, bare `devices` lists). The CLI keeps its hand-written
   commands, but builds the *generated typed params structs* instead of
   `json!`, so the compiler flags the CLI when a method's parameters change.
   `print_human` is untouched.
6. **Out of bounds:** the wire format and validation (`content.rs`, `fit_*`,
   `check_*`), `post-proto`, the server, `store.rs`, and everything under
   `app/`. Do not hand-edit `frb_generated.rs` or `app/lib/src/rust/`.
7. **Irregular methods stay hand-written.** The generator does not need an
   escape hatch for them: a method without `#[api]` is simply not generated,
   and the hand-written and generated halves coexist.

## 4. What was checked before writing this

Verified:

- A strip-only attribute macro accepts doc comments and `#[api(...)]` on
  function parameters, and `#[api(...)]` on struct fields, under
  `#![deny(missing_docs)]` on edition 2024 with syn 2.0.119. The macro in
  section 5.1 is the code that was compiled. rustfmt keeps the parameter docs.
- `flutter_rust_bridge_codegen` 2.13.0 reads the crate through `cargo expand`
  when run from the command line (`src/library/commands/cargo_expand/mod.rs`),
  and `rustc -Zunpretty=expanded` (what `cargo expand` runs) inlines
  `include!`d items into the including module.
- FRB attaches methods to a Dart class by owner type across all parsed
  functions (`generator/api_dart/spec_generator/class/method.rs`), so an
  `impl Client` block outside the struct's own `impl` is supported.
- `cargo check --workspace --all-targets` passes on the starting commit.

Not verified (no Flutter in the environment this was written in):

- An actual `flutter_rust_bridge_codegen generate` run over the result.
  Section 9 says what to expect and section 10 gives the fallback.

## 5. Components

Two new crates under `crates/` (the workspace's `members = ["crates/*"]` picks
them up). Add shared dependency versions to `[workspace.dependencies]` in the
root `Cargo.toml`: `syn = { version = "2", features = ["full", "visit-mut"] }`,
`quote = "1"`, `proc-macro2 = { version = "1", features = ["span-locations"] }`,
`prettyplease = "0.2"`. Use syn 2, which `prettyplease` needs; 2.0.119 is
already in `Cargo.lock`. Both crates use `edition.workspace = true`,
`rust-version.workspace = true`, `publish.workspace = true` and
`[lints] workspace = true` like the others.

### 5.1 `crates/post-api-macros` (proc-macro crate)

One attribute, `#[api]`, that removes the annotations and returns the item
otherwise unchanged. It validates nothing. Dependencies: `syn`, `quote`.

```rust
//! The `#[api]` marker read by `post-apigen`. It only removes itself: the
//! generator, not this macro, gives the annotations their meaning.

use proc_macro::TokenStream;
use quote::ToTokens;
use syn::visit_mut::{self, VisitMut};

struct Strip;

fn is_api(a: &syn::Attribute) -> bool {
    a.path().is_ident("api")
}

impl VisitMut for Strip {
    fn visit_field_mut(&mut self, f: &mut syn::Field) {
        f.attrs.retain(|a| !is_api(a));
        visit_mut::visit_field_mut(self, f);
    }
    fn visit_variant_mut(&mut self, v: &mut syn::Variant) {
        v.attrs.retain(|a| !is_api(a));
        visit_mut::visit_variant_mut(self, v);
    }
    fn visit_fn_arg_mut(&mut self, a: &mut syn::FnArg) {
        // Doc comments are not allowed on parameters once macros have run.
        if let syn::FnArg::Typed(t) = a {
            t.attrs.retain(|a| !is_api(a) && !a.path().is_ident("doc"));
        }
    }
}

/// Marks a `Core` method or a type as part of the local API. See
/// `docs/apigen-prototype.md`.
#[proc_macro_attribute]
pub fn api(_args: TokenStream, item: TokenStream) -> TokenStream {
    if let Ok(mut f) = syn::parse::<syn::ImplItemFn>(item.clone()) {
        Strip.visit_impl_item_fn_mut(&mut f);
        return f.into_token_stream().into();
    }
    match syn::parse::<syn::Item>(item) {
        Ok(mut i) => {
            Strip.visit_item_mut(&mut i);
            i.into_token_stream().into()
        }
        Err(e) => e.to_compile_error().into(),
    }
}
```

`post-core` depends on it and does `use post_api_macros::api;`.

### 5.2 `crates/post-apigen` (library + binary)

Reads `crates/post-core/src/lib.rs`, writes the two generated files.

```
crates/post-apigen/
  src/lib.rs        pub fn generate(inputs) -> Result<Outputs>; path constants
  src/model.rs      ApiModel { types, methods } and their parts
  src/parse.rs      syn::File -> ApiModel, with validation
  src/types.rs      the type table (section 6.3)
  src/emit_api.rs   ApiModel -> api.generated.rs
  src/emit_ffi.rs   ApiModel -> client.generated.rs
  src/main.rs       the command line
  tests/golden.rs, tests/errors.rs, tests/up_to_date.rs
  tests/fixtures/sample.rs
  tests/golden/sample.api.rs, tests/golden/sample.ffi.rs
```

Command line:

```sh
cargo run -p post-apigen             # write both generated files
cargo run -p post-apigen -- --check  # write nothing; exit 1 and name the stale file if output would differ
```

The workspace root is found from `env!("CARGO_MANIFEST_DIR")/../..`. Inputs and
outputs are constants in `lib.rs`; the input list is a slice so more core files
can be added later (some API types live in `picture.rs`, `update.rs`).

Emission: build a `proc_macro2::TokenStream` with `quote!`, parse it as a
`syn::File`, format it with `prettyplease::unparse`, and prepend this header:

```rust
// @generated by post-apigen from crates/post-core/src/lib.rs.
// Do not edit: change the #[api] declarations there, then run `cargo run -p post-apigen`.
```

Output must be deterministic: types in source order, then methods in source
order. Doc comments are emitted as `#[doc = "..."]` in the token stream;
prettyplease prints them as `///`.

## 6. The annotation language

### 6.1 Types

```rust
/// One of this account's devices.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device { … }

/// A single-use invite code, made by a host.
#[api(rpc_name = "InviteCode")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The code. Shown once: the server keeps only its hash.
    pub code: String,
    /// The address to sign up at with it: this account's server.
    pub server: String,
    /// When it stops working (Unix ms).
    #[api(unix_ms)]
    pub expires_at_ms: u64,
    /// It registers a bot account.
    pub bot: bool,
}
```

| Key | On | Meaning | Default |
|---|---|---|---|
| `rpc_name = "X"` | struct | the type's name in `post-api` | the Rust name |
| `ffi_name = "X"` | struct | the type's name in `post-ffi` (and so in Dart) | the Rust name + `Info` |
| `unix_ms` | `u64` field | a time; the bridge type is `i64` (Dart `int`, not `BigInt`), converted with the existing `ms()` helper | |

Structs with named, `pub` fields only. Enums are out of scope for the
prototype: `#[api]` on an enum is an error saying so.

Reorder `Invite`'s fields in core to `code, server, expires_at_ms, bot` as
shown. That is the order both existing surfaces use; generated structs follow
core's field order, and keeping the old order keeps the JSON and the bridge's
wire encoding byte-for-byte the same.

### 6.2 Methods

```rust
/// Name one of this account's devices, given its ID or an unambiguous
/// prefix. The name is shared only with the account's own devices.
#[api(needs = connection)]
pub async fn rename_device(
    &self,
    /// Device ID, or an unambiguous prefix of at least four characters.
    device: &str,
    /// The new name.
    name: &str,
) -> Result<()> { … }

/// Make a single-use invite code for someone to create an account with
/// (hosts only), valid for `days` (1 to 30).
#[api(needs = connection)]
pub async fn create_invite(
    &self,
    /// For a bot account.
    #[api(default)]
    bot: bool,
    /// Days until it expires, 1 to 30; default 7.
    #[api(default = 7)]
    days: u32,
) -> Result<Invite> { … }
```

| Key | On | Meaning |
|---|---|---|
| `needs = local \| connection \| registration` | method, required | becomes `Needs::Local` etc. in the method table |
| `/// …` | parameter, required | the parameter's description in the JSON schema |
| `default` | parameter | optional over JSON-RPC, `Default::default()` when absent: the params field keeps its type and gets `#[serde(default)]` |
| `default = <literal>` | parameter | optional over JSON-RPC with that value when absent: the params field is `Option<T>` with `#[serde(default)]`, and the call uses `.unwrap_or(<literal>)` |

Defaults apply to JSON-RPC only. The bridge method always takes every
parameter (Dart call sites pass them all today).

Use `needs` exactly as the existing `methods!` rows say: `devices` is `local`,
the other eight are `connection`. Take the parameter descriptions from the
existing params structs in `post-api` (`RenameDeviceParams`, `DeviceParams`,
`CodeParams`, `CreateInviteParams`, `SetAdminParams`, `ConversationParams`).

The method's doc comment becomes the bridge method's doc comment in full, and
its first paragraph (whitespace collapsed to single spaces) becomes the
JSON-RPC `summary`.

Reserved for later, not in the prototype: `rpc_name` / `ffi_name` on methods,
and a way to expose a method on one surface only.

### 6.3 The type table

The generator supports exactly these; anything else is an error (6.4).

Parameters:

| Core parameter type | Params field (`post-api`) | Bridge argument (`post-ffi`) | Passed to core as |
|---|---|---|---|
| `&str` | `String` | `String` | `&p.x` / `&x` |
| `String` | `String` | `String` | by value |
| `bool`, `u32`, `u64` | same | same | by value |
| `ConversationId`, `MessageId`, `CallId` | `String` (hex) | `String` (hex) | api: `id(&p.x)?`; ffi: `x.parse()?` |
| `&[String]` | `Vec<String>` | `Vec<String>` | `&p.x` / `&x` |
| `Option<&str>` | `Option<String>` | `Option<String>` | `.as_deref()` |

Results (`Result` is `post_core::Result`; sync and `async` methods are both
supported, `.await` emitted only for `async`):

| Core return type | `post-api` result | `post-ffi` result |
|---|---|---|
| `Result<()>` | `Done` | `()` |
| `Result<T>`, `T` an `#[api]` type | `T`'s rpc type | `T`'s ffi type |
| `Result<Vec<T>>` | `Vec<…>` | `Vec<…>` |
| `Result<Option<T>>` | `Option<…>` | `Option<…>` |

Fields of `#[api]` structs: `String`, `bool`, `u32`, `u64` (with or without
`unix_ms`), another `#[api]` type, and `Vec<…>` / `Option<…>` of those.

Only the first four parameter rows, the first three result rows and
`String` / `bool` / `u64` fields are needed by the slice. Implement the whole
table and cover the rest in the fixture; it is small, and the next methods
converted will need it.

### 6.4 Errors

The generator fails with `file:line:column: message` (and a non-zero exit) on:

- an unknown key in `#[api(...)]`, or a key on the wrong kind of item;
- a method that is not `pub`, is generic, or is not in an inherent
  `impl Core` block; a missing or unknown `needs`;
- a parameter, field or return type not in the table. The message names the
  type and says: leave the method hand-written, or extend the type table;
- a type used by an `#[api]` method or field that is not itself `#[api]`;
- a missing doc comment on an `#[api]` method, type, field or parameter
  (generated code must satisfy the workspace's `missing_docs` lint);
- `unix_ms` on a field that is not `u64`; `default = …` whose literal does not
  suit the parameter type;
- two types with the same `rpc_name` or the same `ffi_name`; a tuple or unit
  struct; a non-`pub` field; `#[api]` on an enum.

## 7. What gets generated

### 7.1 `crates/post-api/src/api.generated.rs`

Included at the end of `lib.rs` with `include!("api.generated.rs");`. It may
use what `lib.rs` already has in scope: `core`, `Core`, `Value`, `Serialize`,
`Deserialize`, `JsonSchema`, `MethodInfo`, `Needs`, `ApiError`, `Done`,
`NoParams`, and the private helpers `schema`, `params`, `ok`, `id`.

For each type: the struct and its conversion.

```rust
/// A single-use invite code, made by a host.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct InviteCode {
    /// The code. Shown once: the server keeps only its hash.
    pub code: String,
    /// The address to sign up at with it: this account's server.
    pub server: String,
    /// When it stops working (Unix ms).
    pub expires_at_ms: u64,
    /// It registers a bot account.
    pub bot: bool,
}

impl From<core::Invite> for InviteCode {
    fn from(v: core::Invite) -> Self {
        Self { code: v.code, server: v.server, expires_at_ms: v.expires_at_ms, bot: v.bot }
    }
}
```

Fields that are `#[api]` types convert with `.into()`, `Vec`s with
`.into_iter().map(Into::into).collect()`, `Option`s with `.map(Into::into)`.

For each method with parameters: a params struct named `<MethodName>Params` in
PascalCase (`RenameDeviceParams`, `RemoveDeviceParams`, `LinkInspectParams`,
`LinkApproveParams`, `CreateInviteParams`, `SetAdminParams`,
`LeaveGroupParams`). One per method; they are not shared. Methods without
parameters use the existing `NoParams`.

```rust
/// Parameters of `create_invite`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteParams {
    /// For a bot account.
    #[serde(default)]
    pub bot: bool,
    /// Days until it expires, 1 to 30; default 7.
    #[serde(default)]
    pub days: Option<u32>,
}
```

One private function per method, the table, and the dispatcher:

```rust
async fn call_devices(core: &Core, p: Value) -> Result<Value, ApiError> {
    params::<NoParams>(p)?;
    ok(core.devices().await?.into_iter().map(Device::from).collect::<Vec<_>>())
}

async fn call_create_invite(core: &Core, p: Value) -> Result<Value, ApiError> {
    let p: CreateInviteParams = params(p)?;
    ok(InviteCode::from(core.create_invite(p.bot, p.days.unwrap_or(7)).await?))
}

async fn call_set_admin(core: &Core, p: Value) -> Result<Value, ApiError> {
    let p: SetAdminParams = params(p)?;
    core.set_admin(id(&p.conversation)?, &p.username, p.admin).await?;
    ok(Done {})
}

/// The generated methods, in the order post-core declares them.
const GENERATED_METHODS: &[MethodInfo] = &[
    MethodInfo {
        name: "devices",
        summary: "This account's devices, oldest first.",
        needs: Needs::Local,
        params: schema::<NoParams>,
        result: schema::<Vec<Device>>,
    },
    // …
];

/// Run a generated method. The caller has checked `name` is one of them.
async fn call_generated(core: &Core, name: &str, p: Value) -> Result<Value, ApiError> {
    match name {
        "devices" => call_devices(core, p).await,
        // …
        _ => Err(ApiError { code: -32601, message: format!("unknown method {name}") }),
    }
}
```

Hand edits to `crates/post-api/src/lib.rs`:

- Delete `Device`, `LinkRequest`, `InviteCode`, `RenameDeviceParams`,
  `DeviceParams`, `CodeParams`, `CreateInviteParams`, `SetAdminParams`, the
  nine rows in `methods!` and the nine arms in `call` (`link_inspect` and
  `link_approve` share one). Keep `ConversationParams`, `NoParams` and `Done`:
  other methods use them.
- Have `methods!` emit `const HANDWRITTEN_METHODS` instead of `pub const
  METHODS`, and add
  `pub fn methods() -> impl Iterator<Item = &'static MethodInfo>` chaining it
  with `GENERATED_METHODS`. Use it in `method()`, `openrpc()` and the test
  `every_method_dispatches_and_has_a_schema`. Nothing outside the crate uses
  `METHODS`.
- At the top of `call`: if `name` is in `GENERATED_METHODS`, return
  `call_generated(core, name, p).await`.

### 7.2 `crates/post-ffi/src/api/client.generated.rs`

Included at the end of `client.rs` with `include!("client.generated.rs");`. It
may use what `client.rs` has in scope: `core`, `Client` (and its private `core`
field), `PostError`, and the private helper `ms`.

```rust
/// A single-use invite code, made by a host.
pub struct InviteInfo {
    /// The code. Shown once: the server keeps only its hash.
    pub code: String,
    /// The address to sign up at with it: this account's server.
    pub server: String,
    /// When it stops working (Unix ms).
    pub expires_at_ms: i64,
    /// It registers a bot account.
    pub bot: bool,
}

impl From<core::Invite> for InviteInfo {
    fn from(v: core::Invite) -> Self {
        Self { code: v.code, server: v.server, expires_at_ms: ms(v.expires_at_ms), bot: v.bot }
    }
}

impl Client {
    /// This account's devices, oldest first.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>, PostError> {
        Ok(self.core.devices().await?.into_iter().map(Into::into).collect())
    }

    /// Make a single-use invite code for someone to create an account with
    /// (hosts only), valid for `days` (1 to 30).
    pub async fn create_invite(&self, bot: bool, days: u32) -> Result<InviteInfo, PostError> {
        Ok(self.core.create_invite(bot, days).await?.into())
    }

    /// As an admin: make a member an admin, or stop them being one. A group
    /// always keeps at least one admin.
    pub async fn set_admin(&self, conversation: String, username: String, admin: bool) -> Result<(), PostError> {
        Ok(self.core.set_admin(conversation.parse()?, &username, admin).await?)
    }
}
```

Bridge structs carry no derives, like the existing ones. Bridge methods are
always `async`. Type names, field names and types, method names and argument
names and types must come out identical to the hand-written ones being
replaced; that is what keeps `frb_generated.rs` and Dart valid.

Hand edits to `crates/post-ffi/src/api/client.rs`: delete `DeviceInfo`,
`LinkRequestInfo`, `InviteInfo` and the nine methods from `impl Client`; add
the `include!`.

### 7.3 `crates/post-cli/src/main.rs`

Where the CLI builds parameters for the nine methods (the arms in `to_call`,
and the two `api_call(core, "link_inspect" | "link_approve", json!(…))` calls
in the link flow), build the generated struct instead:

```rust
Command::Devices { cmd: Some(DevicesCommand::Rename { device, name }) } => {
    ("rename_device", typed(post_api::RenameDeviceParams { device: device.clone(), name: name.clone() }))
}
Command::Invite { bot, days } => ("create_invite", typed(post_api::CreateInviteParams { bot: *bot, days: Some(*days) })),
```

with a small helper `fn typed<P: serde::Serialize>(p: P) -> Value`. Commands,
flags, prompts and `print_human` do not change.

## 8. Order of work

Each step ends in something that compiles or a test that passes.

0. **Baseline.** Before any change, save the schema:
   `cargo run -q -p post-cli -- schema > /tmp/schema.before.json`. Note which
   tests pass (`cargo test --workspace --release`, as the README runs them;
   unoptimised post-quantum handshakes are slow).
1. **`post-api-macros`** as in 5.1, with one integration test
   (`tests/strip.rs`) in which the annotated struct and methods from section 6
   compile under `#![deny(missing_docs)]`.
2. **`post-apigen`**: model, parser, type table, the two emitters, the command
   line. Write `tests/fixtures/sample.rs` first; it is parsed, never compiled,
   and should contain every row of the type table, both `default` forms,
   `unix_ms`, both renames, a nested `#[api]` type, a sync method, and an
   unannotated method and struct that must not appear in the output.
   `tests/golden.rs` compares the output with `tests/golden/sample.{api,ffi}.rs`
   and rewrites them when `APIGEN_UPDATE_GOLDEN=1`. `tests/errors.rs` checks
   each error in 6.4 against a small source string.
3. **Annotate `post-core`**: the three types (reordering `Invite`) and the nine
   methods. `cargo check -p post-core` and its tests still pass; nothing else
   has changed yet.
4. **Generate, then wire `post-api`** (7.1). The compiler reports each
   hand-written item still to delete as a duplicate definition.
5. **Wire `post-ffi`** (7.2). `cargo check -p post_ffi` must pass *without*
   regenerating `frb_generated.rs`. If it does not, a generated name or
   signature differs from the one it replaced: fix the generator or the
   annotation, not `frb_generated.rs`.
6. **CLI** (7.3).
7. **`tests/up_to_date.rs`** in `post-apigen`: generate in memory from the real
   inputs and compare with the two committed files, failing with "run `cargo
   run -p post-apigen`". This makes `cargo test --workspace` the staleness
   check.
8. **README**: under "Development", next to the note about regenerating the
   bridge, three lines on `#[api]`, `cargo run -p post-apigen`, and that
   `flutter_rust_bridge_codegen generate` follows when a bridge type or method
   changed.

Commit in the repository's style (`area: one sentence saying what changed`).

## 9. Acceptance

1. `cargo run -p post-apigen -- --check` exits 0. After hand-editing a
   generated file it exits 1 and names the file.
2. `cargo test --workspace --release` passes: the new golden, error and
   up-to-date tests, and everything that passed at baseline (notably
   `post-core/tests/e2e.rs` and `post-cli/tests/daemon.rs`).
3. `cargo clippy --workspace --all-targets` reports nothing new.
4. **Schema unchanged in substance.** `post schema` after, compared with the
   baseline, differs only in: the order of methods; `summary`, `description`
   and `title` text (now taken from core's doc comments and the new params
   struct names). One way to compare:

   ```sh
   norm='.methods |= (map(del(.summary)) | sort_by(.name)) | walk(if type == "object" then del(.description, .title) else . end)'
   diff <(jq -S "$norm" /tmp/schema.before.json) <(cargo run -q -p post-cli -- schema | jq -S "$norm")
   ```

   The diff must be empty. Any other difference (a parameter's name, type or
   required-ness, a result's shape) is a bug.
5. `git diff --stat` shows no changes under `app/` and none to
   `crates/post-ffi/src/frb_generated.rs`.
6. The hand-written versions are gone: no `pub struct DeviceInfo`,
   `LinkRequestInfo`, `InviteInfo` or the nine methods in `client.rs`; no
   `pub struct Device`, `LinkRequest`, `InviteCode` or the listed params
   structs, table rows and `call` arms in `post-api/src/lib.rs`.
7. **Bridge regeneration, if Flutter and `flutter_rust_bridge_codegen` 2.13.0
   are available** (it installs `cargo-expand` itself): run
   `flutter_rust_bridge_codegen generate` in `app/`, then `flutter analyze`.
   Expected: the only differences in `frb_generated.rs` and
   `app/lib/src/rust/` are doc comments. Commit them. If the tools are not
   available, say so in the report; it is the one check left for the
   repository's owner.
8. **Payoff check** (do not commit): add a field to core's `Device`
   (`/// Demo.` `pub demo: bool`, set to `false` where `Core::devices` builds
   it), run `cargo run -p post-apigen`, then
   `cargo check -p post-core -p post-api -p post-cli`. It should pass with
   hand edits in `post-core` only. (`post_ffi` will not compile until the
   bridge is regenerated, because `frb_generated.rs` builds `DeviceInfo`
   field by field; that is expected.) Record `git diff --stat`, then revert.

## 10. If something does not work

- **FRB does not see the included bridge code** (step 9.7 drops the nine
  methods or three types from Dart). Fallback: instead of `include!`, have the
  generator replace the text between two marker comments in `client.rs`
  (`// <post-apigen>` … `// </post-apigen>`), and have `--check` and the
  up-to-date test compare that region. Everything else stays as designed.
- **Doc comments on parameters cause trouble** in some tool. Fallback: move
  them into the method attribute, `#[api(needs = connection, params(device =
  "…", name = "…"))]`, and drop the `doc` stripping from the macro.
- **A slice method does not fit the table.** Do not bend the generator around
  it: leave that method hand-written, convert the rest, and report it.

## 11. Report back

The point of the prototype is to decide whether to convert the other 66
methods, so finish with:

- lines removed from `post-api` and `post-ffi`, lines added to `post-core`
  (annotations), and the size of the generator and its tests;
- the payoff check's `git diff --stat`;
- which acceptance items passed, and for 9.7 whether it was run;
- anything in the annotation language that was awkward, and any core method
  you looked at that clearly will not fit (and why);
- whether either fallback in section 10 was needed.

## 12. Not in the prototype

Generating CLI commands or `print_human`; enums, events and the
`'online'` / `'dm'` strings; methods outside the slice; `store.rs` row
mapping; any Dart or UI change; a task runner or CI job that runs the
generator (the up-to-date test covers staleness for now).
