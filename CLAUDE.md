# Post

An end-to-end encrypted messenger: a Rust core and server, a Flutter app.

- **Read [`docs/architecture.md`](docs/architecture.md) first.** It says what
  the code is being reshaped into, the rules that keep it that way, and which
  phase is under way. Each phase has its own build document in `docs/`.
- The protocol is [`DESIGN-v2.md`](DESIGN-v2.md). It does not change in the
  refactor.

## Commands

```sh
scripts/fmt.sh          # format Rust and the app's hand-written Dart
scripts/check.sh        # what CI checks: formatting, clippy (-D warnings), flutter analyze
scripts/test.sh         # what CI tests: every Rust test, the app's unit tests
scripts/gen.sh          # regenerate every generated file
scripts/dev-server.sh   # a local server; scripts/dev-account.sh NAME ... acts as a test account
app/tool/screens.sh     # screenshots of the main screens (integration tests, opens windows)
```

Tests run on debug builds whose dependencies are optimised; there is no need
for `--release`.

## Rules in brief

- **Generated files** (`frb_generated.*`, `app/lib/src/rust/`, and from phase
  1 anything marked `@generated`) are committed, regenerated with
  `scripts/gen.sh`, and never edited or formatted by hand.
- **Hand-written on purpose:** the wire format and validation
  (`post-core/src/content.rs`, `fit_*`, `check_*`), `post-proto`, database
  migrations and the engine. No generator writes them; changes there are
  security-relevant.
- **The local API** (from phase 1): a method is in it if and only if it is a
  `Core` method marked `#[api]`; API types are core's own types. No copies,
  no hand-written side doors.
- **The app** (from phase 3): features read from `data/` and call `PostApi`,
  never the bridge; colours, text styles, radii and spacing come from `kit/`;
  files stay under about 500 lines.

## Working here

- Work happens on phase branches (now `refactor`), merged into `main` when a
  phase is done. Commits: `area: one sentence saying what changed`, one
  feature or fix each, building and passing `scripts/check.sh` and the
  relevant tests.
- Pure reformatting goes in a commit of its own, listed in
  `.git-blame-ignore-revs`.
