# Phase 1: one generated local API

Build document for phase 1 of [`architecture.md`](architecture.md); read that
first. This replaces the earlier prototype design (two generated surfaces,
nine methods), which is gone.

## 1. Outcome

- `Core`'s methods and types, marked `#[api]`, are the only declaration of the
  local API.
- `post-apigen` generates from them: the JSON dispatch in `post-api`, an
  OpenRPC file, a typed Dart client, and fixtures that prove Rust and Dart
  agree on the JSON.
- `post-api` holds no hand-written types or mappers. The CLI and the daemon
  run on the generated dispatch.
- `post-ffi` gains two functions, `call` and `events_json`. The app reaches
  the API through the generated Dart client; one screen is moved to it as
  proof. The old typed bridge stays for the other screens until phase 4.

The work is in three milestones (section 11). The tree builds and its tests
pass at the end of each.

## 2. Decisions (made; do not reopen)

1. **Source of truth:** `#[api]` on the real `Core` methods and core types.
   API method name = Rust method name. No renames, no per-surface options.
2. **Core types are the API types.** `#[api]` adds the serde derives and fixes
   the JSON form. Nobody writes `#[serde(...)]` on an API type.
3. **Methods must fit the type table.** Where a `Core` method does not, its
   facade signature changes (section 9 lists the known cases). Engine logic
   does not change.
4. **A separate generator writes committed files.** It is not a proc macro
   expanding in other crates. The only proc macro is `#[api]` itself.
5. **Bytes and secrets stay off JSON:** pictures (as bytes) and call media
   keys remain direct bridge functions.
6. **Hand-written on purpose, untouched here:** the wire format and validation
   (`content.rs`, `fit_*`, `check_*`), `post-proto`, the server, `store.rs`.
7. **The API is unreleased**, so names and shapes change freely in this phase.
   `post_api::VERSION` stays `post.v1`.

## 3. Checked before writing this

Verified in a throwaway crate (edition 2024, syn 2.0.119):

- The macro in section 6 compiles and behaves as specified: doc comments and
  `#[api(...)]` on parameters and fields are accepted and removed under
  `#![deny(missing_docs)]`; the JSON it produces matches section 4
  (`{"id":"ab","at":5,"name":null}`, `"do_not_disturb"`,
  `{"type":"message_deleted","conversation":"c","message_id":"m"}`,
  `{"type":"lagged"}`); a missing optional field reads as `None`, a field
  marked `default` may be left out, an unknown field is refused.
- rustfmt keeps doc comments on parameters.
- `cargo check --workspace --all-targets` passes on the starting commit.

From `flutter_rust_bridge_codegen` 2.13.0's source: it reads the crate through
`cargo expand` and attaches methods to a Dart class by owner type, so adding
two methods to `Client` needs nothing special.

Not verified (no Flutter where this was written): anything that runs Dart.
Milestone 3 needs Flutter and `flutter_rust_bridge_codegen` 2.13.0.

## 4. The JSON form

Fixed by `#[api]`; the generator and the Dart client assume exactly this.

| Rust | JSON |
|---|---|
| struct | object; keys are the field names as written (snake_case); unknown keys refused |
| `Option<T>` field | `null` when none; may be left out when read |
| field marked `#[api(default)]` | may be left out when read (`Default::default()`) |
| enum with only unit variants | the variant name in snake_case, as a string: `"do_not_disturb"` |
| enum with fields | object with `"type": "<variant in snake_case>"` plus the variant's fields. Variants are unit or have named fields; no tuple variants |
| `ConversationId`, `MessageId`, `CallId` | 32 hex characters |
| `bool`, `u32`, `u64`, `String` | as JSON |
| `Vec<T>` | array |
| a method's parameters | one object, a key per parameter; unknown keys refused |
| a method returning `()` | `null` |

Add `Serialize` / `Deserialize` to the IDs inside the `hex_id!` macro in
`post-core/src/lib.rs`, through their existing `Display` and `FromStr`.

## 5. The annotation language

```rust
/// A single-use invite code, made by a host.
#[api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invite {
    /// The code. Shown once: the server keeps only its hash.
    pub code: String,
    /// When it stops working (Unix ms).
    #[api(unix_ms)]
    pub expires_at_ms: u64,
    …
}

/// Make a single-use invite code for someone to create an account with
/// (hosts only), valid for `days` (1 to 30).
#[api(needs = connection)]
pub async fn create_invite(
    &self,
    /// For a bot account.
    #[api(default)]
    bot: bool,
    /// Days until it expires, 1 to 30.
    #[api(default = 7)]
    days: u32,
) -> Result<Invite> { … }
```

| Key | On | Meaning |
|---|---|---|
| (none) | struct, enum | the type is part of the API |
| `needs = local \| connection \| registration` | method, required | whether the caller must be connected first (as `Needs` today) |
| `/// …` | everything, required | types, fields, variants, methods and parameters all need a doc comment; parameter docs become the parameter descriptions |
| `unix_ms` | `u64` or `Option<u64>` field or parameter | a time. JSON is unchanged; in Dart it is a `DateTime`, and a trailing `_ms` is dropped from the name (`expires_at_ms` becomes `expiresAt`) |
| `default` | field or parameter | optional when read; `Default::default()` when absent |
| `default = <literal>` | parameter | optional when read; that value when absent |

Mark every time in the API `unix_ms`. A method's doc comment is used in full
for Dart, and its first paragraph (whitespace collapsed) as the OpenRPC
summary.

### The type table

Anything outside this table is a generator error.

| Where | Rust | In the params struct | Dart |
|---|---|---|---|
| parameter | `&str`, `String` | `String` | `String` |
| parameter | `bool`, `u32`, `u64` | same | `bool`, `int`, `int` |
| parameter | `u64` with `unix_ms` | `u64` | `DateTime` |
| parameter | an ID | the ID type | `String` |
| parameter | `&[String]` | `Vec<String>` | `List<String>` |
| parameter | `T` or `&T`, `T` an `#[api]` type | `T` | `T` |
| parameter | `Option<…>` of any of the above (`Option<&str>` included) | `Option<…>` | nullable |
| field | `String`, `bool`, `u32`, `u64`, an ID, an `#[api]` type, and `Option` / `Vec` of those | | as above; `Vec<T>` is `List<T>` |
| return | `()`, `bool`, `u32`, `u64`, `String`, an `#[api]` type, `Option` / `Vec` of those; alone or inside `Result` | | `Future<…>`; `()` is `Future<void>` |

Methods may be `async` or not. `Result` is `post_core::Result`.

### Generator errors

`file:line:column: message`, non-zero exit, on:

- an unknown key, or a key where it does not apply;
- an `#[api]` method that is not `pub`, is generic, or is not in an inherent
  `impl Core` block; a missing or unknown `needs`;
- a type outside the table, naming it; a type used by the API that is not
  itself `#[api]`;
- a missing doc comment anywhere the table above requires one;
- a tuple or unit struct, a non-`pub` field, a tuple variant;
- a name that is a Dart reserved word once converted, or two API types or two
  methods with the same name.

## 6. `crates/post-api-macros`

A proc-macro crate with the one attribute. It removes the annotations, adds
the serde derives and the JSON form of section 4, and validates nothing else.
This is the code that was compiled for section 3:

```rust
//! The `#[api]` marker read by `post-apigen`. It gives API types their JSON
//! form and removes itself; the generator gives the annotations their meaning.

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
        // Doc comments are not allowed on parameters once macros have run.
        if let syn::FnArg::Typed(t) = a {
            t.attrs.retain(|a| !is_api(a) && !a.path().is_ident("doc"));
        }
    }
}

/// Marks a `Core` method or a type as part of the local API. See
/// `docs/phase-1-api.md`.
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
                quote!(#[derive(::serde::Serialize, ::serde::Deserialize)] #[serde(tag = "type", rename_all = "snake_case")] #e).into()
            }
        }
        Ok(other) => syn::Error::new_spanned(other, "#[api] goes on a struct, an enum or a method").to_compile_error().into(),
        Err(e) => e.to_compile_error().into(),
    }
}
```

`post-core` depends on it (`use post_api_macros::api;`); it already depends on
serde with `derive`.

Add to `[workspace.dependencies]`:
`syn = { version = "2", features = ["full", "visit-mut"] }`, `quote = "1"`,
`proc-macro2 = { version = "1", features = ["span-locations"] }`,
`prettyplease = "0.2"`. Use syn 2, which `prettyplease` needs. New crates use
the workspace's edition, rust-version, `publish` and lints like the others.

## 7. `crates/post-apigen`

A library and a binary.

```
src/lib.rs        generate(root) -> Outputs; INPUTS and output paths
src/model.rs      ApiModel { types, methods } and their parts
src/parse.rs      syn::File -> ApiModel, with the errors of section 5
src/emit_rust.rs  api.generated.rs and the fixtures test
src/emit_dart.rs  post_api.dart and the fixtures test
src/emit_schema.rs  openrpc.generated.json and fixtures.generated.json
src/main.rs       the command line
tests/            golden.rs, errors.rs, up_to_date.rs, fixtures/, golden/
```

```sh
cargo run -p post-apigen             # write every generated file
cargo run -p post-apigen -- --check  # write nothing; exit 1 and name any stale file
```

Inputs are a list of core files (start with `src/lib.rs`, `src/picture.rs`,
`src/content.rs`; only `#[api]` items in them are read). The workspace root is
`env!("CARGO_MANIFEST_DIR")/../..`.

Outputs, all committed, all starting with an `@generated` header that names
the command:

| File | Contents |
|---|---|
| `crates/post-api/src/api.generated.rs` | params structs, the method table, `dispatch` |
| `crates/post-api/src/openrpc.generated.json` | the OpenRPC description |
| `crates/post-api/fixtures.generated.json` | one sample JSON value per API type (per variant for enums) |
| `crates/post-api/tests/fixtures.generated.rs` | reads each sample into the core type and writes it back; must be equal |
| `app/lib/api/post_api.dart` | the Dart client |
| `app/test/api_fixtures_test.dart` | the same round trip through the Dart classes |

Rust is built with `quote!`, parsed as a `syn::File` and printed with
`prettyplease::unparse`. Dart and JSON are written as text by the emitters,
already laid out; the Dart files carry `// dart format off` and
`// ignore_for_file: type=lint` so no tool reformats them. Output is
deterministic: types, then methods, each in source order.

The fixtures are the check that the generator's idea of the JSON matches what
serde and the Dart classes really do. Sample values are synthetic (`"s"`, `1`,
`true`, a fixed 32-hex ID, one element per list, both `null` and a value for
each `Option`).

Tests in `post-apigen`:

- `golden.rs`: a fixture source (`tests/fixtures/sample.rs`, parsed, never
  compiled) covering every row of the type table and every annotation, against
  committed outputs in `tests/golden/`; `APIGEN_UPDATE_GOLDEN=1` rewrites them.
- `errors.rs`: each error of section 5 from a small source string.
- `up_to_date.rs`: generates in memory from the real inputs and compares with
  the committed files, failing with "run `cargo run -p post-apigen`". This
  makes `cargo test --workspace` the staleness check.

## 8. What is generated

### `api.generated.rs`

Included at the end of `post-api/src/lib.rs` with `include!`, so it can use
that file's private helpers and a leftover hand-written duplicate is a compile
error.

```rust
/// Parameters of `set_admin`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetAdminParams {
    /// The group.
    pub conversation: ConversationId,
    /// The member's username.
    pub username: String,
    /// True to make them an admin, false to remove the role.
    pub admin: bool,
}

/// Parameters of `create_invite`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteParams {
    /// For a bot account.
    #[serde(default)]
    pub bot: bool,
    /// Days until it expires, 1 to 30.
    #[serde(default = "default_create_invite_days")]
    pub days: u32,
}
fn default_create_invite_days() -> u32 { 7 }

async fn dispatch_set_admin(core: &Core, p: Value) -> Result<Value, CallError> {
    let p: SetAdminParams = params(p)?;
    core.set_admin(p.conversation, &p.username, p.admin).await?;
    Ok(Value::Null)
}

async fn dispatch_devices(core: &Core, p: Value) -> Result<Value, CallError> {
    no_params(p)?;
    result(core.devices().await?)
}

/// Every method, in the order post-core declares them.
pub const METHODS: &[MethodInfo] = &[
    MethodInfo { name: "devices", summary: "This account's devices, oldest first.", needs: Needs::Local },
    // …
];

/// Run one method, with no connection handling.
pub async fn dispatch(core: &Core, name: &str, p: Value) -> Result<Value, CallError> {
    match name {
        "devices" => dispatch_devices(core, p).await,
        // …
        _ => Err(CallError::UnknownMethod(name.to_owned())),
    }
}
```

A params struct is generated for every method with parameters, named
`<Method>Params`. Sync core methods are called without `.await`; methods not
returning `Result` without `?`.

### `openrpc.generated.json`

OpenRPC 1.3.2: for each method its name, summary, `paramStructure: "by-name"`,
parameters (name, description, required, schema) and result schema; every API
type under `components.schemas`, referred to by `$ref`. Schemas follow section
4: objects with `additionalProperties: false`, string enums, `oneOf` on a
`type` constant for enums with fields, IDs as strings with the 32-hex pattern.

### `post_api.dart`

```dart
/// One of this account's devices.
final class Device {
  const Device({required this.id, required this.name, required this.thisDevice});

  factory Device.fromJson(Map<String, Object?> j) =>
      Device(id: j['id'] as String, name: j['name'] as String, thisDevice: j['this_device'] as bool);

  /// Its ID, hex.
  final String id;
  // …

  Map<String, Object?> toJson() => {'id': id, 'name': name, 'this_device': thisDevice};

  @override
  bool operator ==(Object other) => other is Device && other.id == id && other.name == name && other.thisDevice == thisDevice;

  @override
  int get hashCode => Object.hashAll([id, name, thisDevice]);
}

/// What others see of someone.
enum Presence {
  offline('offline'),
  online('online'),
  away('away'),
  doNotDisturb('do_not_disturb');

  const Presence(this.json);
  final String json;
  static Presence fromJson(Object? v) =>
      values.firstWhere((e) => e.json == v, orElse: () => throw FormatException('unknown Presence: $v'));
}

/// Something that happened, for live views.
sealed class Event {
  const Event();
  factory Event.fromJson(Map<String, Object?> j) => switch (j['type']) {
    'message_deleted' => MessageDeletedEvent.fromJson(j),
    // …
    final t => throw FormatException('unknown Event: $t'),
  };
}

final class MessageDeletedEvent extends Event { /* fields, fromJson, toJson, ==, hashCode */ }

/// How the client reaches the core.
abstract interface class PostTransport {
  Future<Object?> call(String method, Map<String, Object?> params);
  Stream<Map<String, Object?>> get events;
}

/// The local API.
abstract interface class PostApi {
  /// Live events.
  Stream<Event> get events;

  /// This account's devices, oldest first.
  Future<List<Device>> devices();

  /// Name one of this account's devices …
  Future<void> renameDevice({required String device, required String name});

  /// Make a single-use invite code …
  Future<Invite> createInvite({bool bot = false, int days = 7});
}

/// [PostApi] over a [PostTransport].
final class RpcPostApi implements PostApi { /* each method: build the map, call, decode */ }
```

Rules: names in lowerCamelCase / UpperCamelCase; a variant's class is
`<Variant><Enum>` (so `Event::Presence` is `PresenceEvent` and does not clash
with the `Presence` enum); `unix_ms` values are `DateTime`
(`DateTime.fromMillisecondsSinceEpoch`, and `millisecondsSinceEpoch` back);
parameters are named, `required` unless they have a default, which is repeated
as the Dart default; list fields compare element by element in `==`. The file
imports nothing from Flutter, so it can be tested with plain Dart.

## 9. Hand-written changes

### `post-core`

Annotate the facade. Every `Core` method is one of:

**API (`#[api]`), with `needs`:**

| Area | Methods |
|---|---|
| Account | `identity` (local), `register`, `begin_link`, `finish_link` (registration), `link_inspect`, `link_approve`, `log_out`, `refresh_keys`, `rename_device`, `remove_device` (connection), `devices` (local) |
| Connection | `is_connected` (local), `sync` (connection) |
| Conversations | `conversations`, `conversation`, `members` (local); `start_dm`, `create_group`, `add_members`, `remove_members`, `rename_group`, `set_admin`, `set_history`, `leave_group`, `mark_read`, `send_typing`, `set_conversation_name` (connection) |
| Messages | `messages`, `message`, `message_versions`, `message_receipts`, `search`, `notices` (local); `send_text`, `edit_message`, `delete_message` (connection) |
| People | `person`, `presence`, `own_presence`, `safety_number`, `read_receipts`, `set_nickname` (local); `set_presence`, `set_verified`, `set_read_receipts`, `set_profile`, `set_banner_fill`, `set_theme` (connection) |
| Calls | `calls`, `start_call`, `join_call`, `leave_call`, `decline_call`, `ring_call` (connection) |
| Announcements | `announcement_summary`, `announcements`, `mark_announcements_read`, `announce`, `plan_maintenance`, `end_maintenance`, `create_invite` (connection) |
| Settings | `setting`, `set_setting` (local) |
| Pictures by file | `set_picture_file`, `save_picture_file` (below) |

**Not API:** `open`, `close`, `closed`, `subscribe`, `connect`, `run`,
`listen`, and the byte and key methods `set_picture`, `picture`,
`picture_scaled`, `call_keys`.

Take parameter descriptions from the existing params structs in `post-api`
before deleting them.

Facade changes needed to fit the table (signatures and plumbing only):

1. **`Event`**: make every variant unit or named-field
   (`ConversationAdded { conversation }`, `MessageAdded { message }`,
   `CallChanged { call }`, `AnnouncementAdded { announcement }`,
   `AnnouncementsChanged { summary }`, and so on), and add
   `Lagged { missed: u64 }`, which the core never emits itself: the daemon and
   the bridge send it when a subscriber fell behind. Update the places that
   build or match events. (Named fields are required, not a style choice: a
   variant wrapping a struct directly would put `type` inside that struct's
   object, which refuses unknown keys.)
2. **`ProfileUpdate`**: drop `banner_fill` and `theme`
   (`Option<Option<Fill>>` does not fit), and add
   `Core::set_banner_fill(fill: Option<Fill>)` and
   `Core::set_theme(theme: Option<Fill>)`, which do what `set_profile` did for
   those fields. Mark `note_keep` `default`.
3. **`Fill`** (declared in `content.rs`, also a wire type): mark it `#[api]`
   there. That adds JSON derives beside the protobuf ones and changes nothing
   on the wire. Mark `picture::PictureKind` too.
4. **Pictures by file**, which the JSON API has today as `set_picture` and
   `save_picture`: move their bodies from `post-api` onto `Core` as
   `set_picture_file(kind: PictureKind, path: Option<&str>)` and
   `save_picture_file(…)`, so bots keep them. Bytes still never cross as JSON.
5. Mark every time field and parameter `unix_ms`.

If another method turns out not to fit, change its facade signature the same
way and note it in the report.

### `post-api`

`lib.rs` shrinks to: `VERSION`; `Needs`; `MethodInfo { name, summary, needs }`;
`ApiError` and `code_of` as now; the new error below; the private helpers the
generated code uses (`params`, `no_params`, `result`); `method(name)`;
`openrpc()` returning the embedded generated file; `call`; and the `include!`.
Delete every hand-written struct, mapper, the `methods!` table, the old `call`
body and the `schemars` dependency.

```rust
/// Why a call failed, before it is given a JSON-RPC code or a sentence.
pub enum CallError {
    /// No such method.
    UnknownMethod(String),
    /// The parameters did not fit.
    InvalidParams(String),
    /// The core refused or failed.
    Core(core::Error),
}
```

with `From<core::Error> for CallError` and `From<CallError> for ApiError`
(-32601, -32602, `code_of`).

`call(core, name, params) -> Result<Value, ApiError>` is `dispatch` plus the
connection handling the CLI and the daemon each do themselves today: connect
first when the method needs a connection and there is none; for a local method
that fails with "not connected", connect and try once more. Remove both copies
from `post-cli`.

### `post-cli`

- `to_call` and the link flow build the generated params structs
  (`post_api::RenameDeviceParams { … }`) instead of `json!`, so a changed
  parameter is a compile error. Method names become core's names (`send` is
  now `send_text`, `call_join` is `join_call`, `whoami` is `identity`, …).
- `print_human` reads each result into the core type
  (`serde_json::from_value::<core::Person>`) before printing, instead of
  indexing JSON by string. Shapes changed in places (colours are numbers,
  presence is `do_not_disturb`, some results are bare values); typed printing
  turns each into a compile error rather than a silently empty line.
  Formatting for people (colours as `#rrggbb`, fills as `#a,#b@135`, times)
  lives here, as does parsing those from arguments.
- The daemon sends `serde_json::to_value(&event)` and `Event::Lagged`.
- Commands, flags and prompts do not change.

### `post-ffi` and the old bridge

Add to `impl Client` in `api/client.rs`:

```rust
/// Call a method of the local API: `params` and the result are JSON text.
pub async fn call(&self, method: String, params: String) -> Result<String, PostError>

/// Live events as JSON text, until the Dart stream or the profile is closed.
pub async fn events_json(&self, sink: StreamSink<String>)
```

`call` uses `post_api::dispatch` (no connection handling: the app's `run()`
keeps the connection). Add `From<post_api::CallError> for PostError` in
`api/error.rs`: `Core(e)` through the existing mapping, the other two as
`ErrorKind::Other` with the message. `events_json` follows the existing
`events`, sending `Event::Lagged` where that sends its catch-all.

Everything else in `client.rs` stays for now and is adjusted only as far as
the core changes above require (the event mapping, `set_banner_fill`,
`set_theme`). Do not log parameters or results.

### The app (milestone 3)

- `app/lib/api/bridge_transport.dart`: a `PostTransport` over `Client.call`
  and `Client.eventsJson`, with `jsonEncode` / `jsonDecode`.
- `AppState` gets `final PostApi api`, built beside `client`.
- Move **Settings › Devices** (`_DeviceSettings` in `screens/settings.dart`)
  to `api`: `devices`, `renameDevice`, `removeDevice`, and
  `api.events.whereType<DevicesChangedEvent>()`. Nothing else moves in this
  phase.

## 10. If something does not work

- **A tool rejects doc comments on parameters** (rustc accepts them only
  because `#[api]` strips them first; watch rust-analyzer and clippy too).
  Move them into the method
  attribute (`#[api(needs = …, params(device = "…"))]`) and drop the `doc`
  stripping from the macro.
- **A core method cannot reasonably fit the table.** Leave it out of the API
  (unannotated), keep whatever the old bridge does for it, and report it.
  Do not add a hand-written entry to the dispatch.
- **JSON turns out measurably slow** (milestone 3's measurement). Report the
  numbers; do not redesign. The likely fix is decoding off the UI isolate.

## 11. Milestones

Commit in the repository's style (`area: one sentence saying what changed`).
Tests are run as the README says until phase 0 changes that.

### M1: the generator, on its fixture

`post-api-macros` and `post-apigen` exist; nothing else has changed.

- `cargo test -p post-api-macros -p post-apigen` passes: the macro test (the
  annotated examples compile under `#![deny(missing_docs)]` and serialise as
  section 4 says), the golden test over a fixture covering the whole table,
  and the error tests.
- The golden Dart output is valid Dart: if `dart` is available,
  `dart analyze` on it is clean.

### M2: the Rust side runs on it

Core annotated and reshaped; `post-api`, `post-cli` and the daemon as in
section 9; `post-ffi` adjusted to still build, with `call` and `events_json`
added.

- `cargo run -p post-apigen -- --check` exits 0.
- `cargo test --workspace --release` passes, including `up_to_date.rs`, the
  generated fixtures test, the core's end-to-end tests and
  `post-cli/tests/daemon.rs` (updated for the new names and shapes).
- `cargo clippy --workspace --all-targets` reports nothing new.
- `grep -c 'pub struct' crates/post-api/src/lib.rs` is at most 2
  (`ApiError`, `MethodInfo`); `schemars` is gone from the workspace.
- `scripts/dev-account.sh` still works for a round of
  `send`, `conversations`, `devices`, `person` against `scripts/dev-server.sh`.
- `git diff --stat` shows nothing under `app/lib` except the new
  `app/lib/api/post_api.dart`.

### M3: the app reaches it (needs Flutter)

- `flutter_rust_bridge_codegen generate` in `app/` adds `call` and
  `eventsJson`; the rest of its diff follows only from the adjusted old
  bridge.
- `flutter analyze` is clean; `flutter test` passes, including
  `test/api_fixtures_test.dart`.
- Settings › Devices works against the dev server through `PostApi`: the list
  loads, renaming and removing work, and linking a device from the CLI
  updates the list.
- Measure `api.messages(limit: 150)` on a conversation with 150 messages:
  the median of 20 calls, in a debug and a release build, split into the
  bridge call and the decoding if practical.
- **Payoff check** (do not commit): add a field to core's `Device`, run the
  generator, and use it in the Devices page. Record `git diff --stat`.

If Flutter is not available, stop after M2 and say so.

## 12. Report back

- For each milestone: which checks passed, and any that could not be run.
- Lines removed from `post-api`, `post-cli` and `post-ffi`; the size of the
  generator, its tests and each generated file.
- Every facade signature changed beyond section 9's list, and why.
- Any method left out of the API, and why.
- The M3 timings and the payoff check's diff stat.
- Anything in the annotation language or the type table that was awkward.

## 13. Not in this phase

Moving any screen but Settings › Devices; deleting the old typed bridge;
splitting `lib.rs`, `engine.rs` or `store.rs`; generating CLI commands;
updates and spellchecking (they stay on the old bridge); `store.rs` row
mapping; the tooling of phase 0.
