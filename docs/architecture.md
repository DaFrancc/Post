# Post: target architecture

Read this first. It says what the codebase is being reshaped into, the rules
that keep it that way, and the order of the work. Each phase has its own build
document; this one stays short and is kept current as phases finish.

The protocol design is in [`DESIGN-v2.md`](../DESIGN-v2.md) and does not
change. This document is about how the code is organised.

## 1. Why

The codebase works, but adding to it is slow. Measured on the starting commit
(`134bbb9`):

- One new profile field (the bio) took about 30 hand edits across 8 Rust files
  before any UI, because the same type is declared by hand in five places and
  the app and the CLI reach the core through two unrelated APIs.
- A screen cannot be looked at without a server, accounts and release
  binaries. There are no widget tests. Every screen loads its own data and
  repeats the same error handling.
- The UI kit is half used: 167 inline `TextStyle`s against 118 uses of the
  shared ones, 16 different corner radii, 17 separate hover widgets.
- Four files are between 1,600 and 3,900 lines.
- No formatter settings are committed, tests are run only in release builds,
  and CI runs only on release tags.

The goal: **a change should be made in one place, and that place should be
obvious.**

| To add… | You edit | Then |
|---|---|---|
| a field to something the app shows | the type in `post-core`, and where core fills it in | `scripts/gen.sh` |
| a method the app or CLI can call | one `#[api]` method on `Core` | `scripts/gen.sh`; call it from Dart; optionally add a CLI command |
| something sent between devices | the wire type, its validation, the migration, the engine: by hand, on purpose | |
| a settings page | one file under `features/settings/`, built from kit widgets | |
| a screen state to look at | one entry in the preview gallery, with fake data | hot reload |

## 2. The shape

```
 post-identity  post-mls  post-proto            unchanged
        \          |         /
         post-server (postd)                    unchanged

 post-core ── the client: engine, storage, and the Core facade
     │         #[api] on Core's methods and types is the one declaration
     │         of the local API
     │
 post-apigen ── reads those annotations, writes:
     ├── post-api/src/api.generated.rs       params structs, method table, dispatch
     ├── post-api/src/openrpc.generated.json the API as a reviewable file
     └── app/lib/api/post_api.dart           the typed Dart client
     │
 post-api ── call(core, "method", json) -> json           (generated dispatch)
     ├── post-cli      commands -> call                   (bots: post daemon)
     └── post-ffi      one bridge function: call, plus the event stream
              │        and the few things that are not JSON (below)
              └── app  PostApi (generated) -> data layer -> features
```

There is **one local API**. The CLI, the JSON-RPC daemon and the app all call
the same methods with the same types.

## 3. What is kept and what is rebuilt

**Kept, logic untouched:** `post-identity`, `post-mls`, `post-proto`,
`post-server`, and the engine and storage logic in `post-core`. This is the
hard, security-sensitive part and its tests (end-to-end, adversarial,
durability) are the safety net for everything else. Its large files are split
by domain (phase 2); behaviour does not change.

**Rebuilt:**

- The API boundary: `post-api` becomes generated dispatch; the hand-written
  copies of every type in `post-api` and `post-ffi` go away; `post-ffi`
  shrinks to a few fixed functions.
- The app's skeleton: the data layer, the UI kit, the folder structure, a
  preview gallery and tests. Screens are then rebuilt on it one at a time.
  The parts that were hard to get right are moved, not rewritten: message
  markup, arrival animations, picture rendering, the window frame, sounds,
  spellchecking, calls.

## 4. Rules

### The local API

1. A method is in the API if and only if it is a `Core` method marked
   `#[api]`. Its API name is the Rust method's name. There are no renames.
2. API types are core's own types, marked `#[api]`. There are no copies of a
   type for JSON or for the app.
3. `#[api]` fixes the JSON form (phase 1 document, "The JSON form"). Nobody
   writes serde attributes on API types by hand.
4. If a method's natural signature does not fit the generator's type table,
   change the signature on the `Core` facade so it does. Do not add a
   hand-written side door.
5. Raw bytes and secret key material never cross as JSON: pictures and call
   media keys stay direct bridge functions.
6. API traffic is not logged in release builds, and never by default: it
   contains message text.
7. The API has not been released, so it changes freely for now. From the
   first release, `openrpc.generated.json` is the contract: a diff to it in a
   pull request is an API change and is reviewed as one.

### Generated code

1. Generated files are committed, start with an `@generated` header, and are
   never edited or formatted by hand.
2. `scripts/gen.sh` regenerates everything. `cargo test` fails if a generated file
   is stale.
3. A generator fails loudly on anything it does not support, naming the file
   and line. It never guesses.

### Hand-written on purpose

The wire format (`post-core/src/content.rs`, `post-proto`), the validation of
what arrives (`fit_*`, `check_*`), database migrations and the engine. These
decide what a hostile peer or server can make a device store and show. No
generator writes them.

### The core's layout (phase 2)

```
crates/post-core/src/
  lib.rs      Core, Error, re-exports
  api/        the facade, by domain: each file holds a domain's #[api] types
              and its `impl Core` block (account, devices, conversations,
              messages, profiles, presence, calls, announcements, settings)
  engine/     protocol work, by the same domains
  store/      schema, migrations, and one file per group of tables
  content.rs, net.rs, …   as now
```

### The app's layout (phases 3 and 4)

```
app/lib/
  api/        post_api.dart (generated) and the bridge transport
  data/       what the app knows, kept current from events; depends only on PostApi
  kit/        design tokens and shared widgets
  features/   one folder per feature (chat, home, settings, profile, calls, …)
  preview/    the gallery: every screen and state, on a fake PostApi
```

1. Features never import the bridge (`src/rust`). They read from `data/` and
   call `PostApi`.
2. Colours, text styles, radii, spacing and durations come from `kit/`. A raw
   `TextStyle(`, `Color(0x…)` or numeric radius outside `kit/` fails a check.
3. Loading, errors and reloading on events are done one way, by the data
   layer, not per screen.
4. A widget another file needs lives in its own file and is public. No file
   passes its private widgets to another as callbacks.
5. Every screen has gallery entries for its states (loading, empty, typical,
   error), and a golden test from those entries.
6. Files stay under about 500 lines; past that, split by widget.

## 5. Phases

| # | Phase | Document | Needs | State |
|---|---|---|---|---|
| 0 | Tooling | below | | done |
| 1 | One generated local API | [`phase-1-api.md`](phase-1-api.md) | | done |
| 2 | Core file layout | to write | 1, for `lib.rs`; `engine.rs` and `store.rs` can be split alongside phase 1 | not started |
| 3 | App foundations: data layer, kit, gallery, tests | to write once phase 1's Dart client exists | 1 | not started |
| 4 | Screens rebuilt on the foundations; the old bridge deleted | to write after 3 | 3 | not started |

The app keeps working throughout. Phase 1 adds the new API beside the old
typed bridge; each screen moves over when it is rebuilt in phase 4, and the
old bridge (`post-ffi/src/api/client.rs`'s typed methods, the 18,000 lines of
`frb_generated.*`) is deleted when the last screen has moved.

Phase 4 carries the most risk. Each rebuilt screen is compared with the old
one (`app/tool/screens.sh`, same states, same window sizes) before the old
screen is deleted.

The work happens on the `refactor` branch, merged into `main` as each phase
finishes. Phases 2 to 4 get their build documents when the phase before them
is done, so they are written against the code as it then is.

### Phase 0: tooling

Small and independent; no design document. Each item is one commit.

1. **Rust formatting.** Commit `rustfmt.toml` with `max_width = 140` and
   `use_small_heuristics = "Max"` (the settings the code was evidently written
   with: they leave the fewest differences). Format once in a commit of its
   own and list that commit in `.git-blame-ignore-revs`. (Done: the settings
   also state edition 2024, so `flutter_rust_bridge_codegen`, which runs
   rustfmt itself, writes `frb_generated.rs` already formatted.)
2. **Dart formatting.** Set `formatter: page_width:` in
   `app/analysis_options.yaml` to the width the code already uses (nearly all
   lines fit 160), and format once the same way. The generated bridge
   (`app/lib/src/rust/`) keeps codegen's format and is excluded from the
   formatter and its check.
3. **Fast test builds.** In the root `Cargo.toml`:
   `[profile.dev.package."*"] opt-level = 3`. Measured: rebuilding the core's
   end-to-end tests after an edit took 29 s in release and 2 s this way, with
   the same 11.5 s test run. (The first build is slower: 224 s against 162 s.)
   Then switch the README, `scripts/` and `app/integration_test/harness.dart`
   from `--release` / `target/release` to the dev profile.
4. **Task scripts** beside the existing ones in `scripts/`, plain bash with
   nothing extra to install: `gen.sh` (all generators), `check.sh`
   (formatters in check mode, clippy, `flutter analyze`), `test.sh`, `fmt.sh`.
   Running things locally stays with `dev-server.sh` and `dev-account.sh`.
5. **CI on pull requests and pushes to `main` and `refactor`:**
   `scripts/check.sh` and `scripts/test.sh`. Not on every branch push, to
   keep Actions minutes down; `scripts/ci-local.sh` is there for anything
   else.
6. **`CLAUDE.md`** at the root: points here, lists the scripts and the rules
   in section 4 in brief.

## 6. Decisions and why

- **Flutter stays.** None of the measured friction comes from Flutter, and the
  design's reason for it holds: one codebase for Linux, Windows and Android,
  with LiveKit's per-participant key API.
- **One API surface, as JSON.** The alternative, generating a second typed
  bridge for the app, keeps two generators chained and regenerates 18,000
  lines on every change. One surface means one generator, an app that cannot
  disagree with the CLI, and a core that is trivial to fake in tests. The cost
  is JSON encoding per call, which is expected to be negligible at this app's
  sizes and is measured in phase 1.
- **Core types are the API types.** The separate JSON and app copies were
  field-for-field duplicates. One type per concept.
- **The protocol core is not rewritten.** It is correct, tested and
  security-critical; rewriting it would not make features faster to add.

## 7. Open

- The app's state library (plain `ChangeNotifier` stores, Riverpod, or
  signals) is decided in the phase 3 document. Rule 1 and rule 3 of the app's
  layout hold whichever is chosen.
