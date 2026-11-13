# Post

A small, private, end-to-end encrypted messenger for an invite-only community of
about 100 people. Post-quantum key agreement throughout, and a server that
cannot read messages or learn who talks to whom.

The design is in [`DESIGN-v2.md`](DESIGN-v2.md); section 17 tracks progress.
The original sketch is kept in [`DESIGN.md`](DESIGN.md).

## Status

Done: identity, the server with metadata protection, the client core, the
`post` command-line client with a JSON-RPC daemon for bots, multiple devices per
account, and group chats. DMs and groups with unlimited edits (every version
kept), deletes, profiles, admins, leaving, weekly key refresh, device linking
and removal, nicknames, typing indicators and presence work end to end.

In progress: the desktop app (Flutter, Linux first). It signs up, links devices,
and shows conversations, formatted messages, members, profiles and settings.
Attachments, push and calls are later phases.

## Contents

1. [Run it on your computer](#run-it-on-your-computer)
2. [Create an account](#create-an-account)
3. [Test accounts to talk to](#test-accounts-to-talk-to)
4. [The command-line client](#the-command-line-client)
5. [Development](#development)
6. [Layout of the repository](#layout-of-the-repository)

## Run it on your computer

### What you need

- **Rust** 1.88 or later, a C compiler, CMake, and OpenSSL development headers
  (for SQLCipher).
- **Flutter** 3.x with Linux desktop support: GTK 3 development files, Clang,
  Ninja and pkg-config (`flutter doctor` lists anything missing).
- **A keyring**: GNOME Keyring or KWallet. The app keeps its database key there.

### 1. Start a local server

```sh
scripts/dev-server.sh
```

This runs a server for development at `ws://127.0.0.1:8080/v1/ws` and keeps
running until you press Ctrl-C. Its data lives in `.dev/server/` (ignored by
git). Leave it running in its own terminal.

### 2. Build and open the app

```sh
cd app
flutter build linux --release
build/linux/x64/release/bundle/post
```

A build from the repository runs from where it is built. It does not add
itself to your app launcher and does not install updates: pull and build
again instead. Releases do both: install one with its `install.sh` (see
[App updates](deploy/README.md#app-updates)) and it updates itself from your
server.

To see a development build's name and icon on its window under Wayland, it
needs a launcher entry: `linux/packaging/install-dev.sh release` adds one, and
`--uninstall` removes it. It refuses while a release is installed, whose
entry it would replace.

## Create an account

Accounts are by invite only. With the local server running:

1. **Get an invite code.** In another terminal:

   ```sh
   scripts/dev-server.sh invite
   ```

   It prints a 32-character code. Each code works once, for 7 days.

2. **Sign up in the app.** Choose **Create an account** and fill in:
   - **Server:** `ws://127.0.0.1:8080/v1/ws`
   - **Invite code:** the code from step 1
   - **Username:** 3 to 32 lowercase letters, digits and `_`, starting with a
     letter. It cannot be changed later.

   Then press **Create account**.

### Start over with a new account

The app holds one account. To replace it, close the app, delete its profile, and
sign up again with a new invite:

```sh
rm -rf ~/.local/share/com.dafrancc.post/profile
```

If you reset the server too (`scripts/dev-server.sh reset`), every account on it
is gone, so delete the app's profile as well.

### A second account beside your own

To run another account at the same time, for example to message yourself, give
the app its own profile folder and a key file instead of the keyring:

```sh
B=target/debug
cargo build -p post-cli
$B/post keygen .dev/second.key
POST_PROFILE=.dev/second POST_DB_KEY_FILE=.dev/second.key \
  app/build/linux/x64/release/bundle/post
```

A second window opens, and you sign up in it with its own invite.

### Add this account to another device

On the new device, choose **Link to my account**. It shows a code; enter that
code on a device already signed in (**Settings → Devices → Link a device**).

## Test accounts to talk to

`scripts/dev-account.sh` acts as a test account through the command-line
client. An account that does not exist yet is created with a fresh invite the
first time you use it:

```sh
scripts/dev-account.sh alice send --to YOUR_USERNAME "hi from alice"
scripts/dev-account.sh bob send --to YOUR_USERNAME "hi from bob"
scripts/dev-account.sh alice group create climbing YOUR_USERNAME bob
scripts/dev-account.sh alice conversations
```

Any `post` command works after the name. Their profiles live in
`.dev/accounts/`. Do not run commands as an account while the app is open as
that same account.

## The command-line client

`post` is a full client, used by bots and scripts as well as for testing. Each
profile is a folder with a key file. Without the dev scripts:

```sh
cargo build --release
B=target/release
$B/post keygen alice.key
$B/post --profile alice --db-key-file alice.key register \
  --server ws://127.0.0.1:8080/v1/ws --invite INVITE_CODE --username alice

$B/post --profile alice --db-key-file alice.key send --to bob "hello"
$B/post --profile alice --db-key-file alice.key sync
$B/post --profile alice --db-key-file alice.key conversations
```

`post --help` lists every command. `POST_PROFILE` and `POST_DB_KEY_FILE` can
replace `--profile` and `--db-key-file`.

**Link a second device**, from the command line: on the new device run

```sh
$B/post keygen phone.key
$B/post --profile phone --db-key-file phone.key link new \
  --server ws://127.0.0.1:8080/v1/ws --name phone
```

It prints a code and waits. Enter the code on the existing device:

```sh
$B/post --profile alice --db-key-file alice.key link add XXXXX-XXXXX-XXXXX-XXXXX-XXXXX
```

**Bots.** `post daemon --stdio` (or `--socket PATH`) serves the same methods
over JSON-RPC 2.0, one JSON object per line, with live `event` notifications
after `subscribe`. `post schema` prints the OpenRPC description.

`ws://` is for local testing only; real servers use `wss://` behind Caddy. See
[`deploy/README.md`](deploy/README.md) to run a real server.

## Development

```sh
cargo test --workspace                  # Rust tests, end to end included
cargo clippy --workspace --all-targets  # lints
cd app
flutter analyze && flutter test         # Dart checks and unit tests
tool/screens.sh                         # screenshots of the main screens
tool/timing.sh                          # frame timings of the welcome buttons
```

`tool/screens.sh` and `tool/timing.sh` drive the real app against a throwaway
server with its own test accounts; they never touch your profile, keyring or dev
server. Screenshots land in `app/build/screens/`.

After changing the app's Rust bridge (`crates/post-ffi/src/api`), regenerate its
Dart bindings with `flutter_rust_bridge_codegen generate` (version 2.13.0) in
`app/`.

## Layout of the repository

| Path | What |
|---|---|
| `crates/post-identity` | Hybrid Ed25519 + ML-DSA-65 signatures, device chains, safety numbers |
| `crates/post-proto` | Client-server wire protocol |
| `crates/post-mls` | MLS settings shared by server and clients (hybrid ML-KEM-768 + X25519 suite) |
| `crates/post-server` | `postd`, the server |
| `crates/post-core` | The client core: keys, MLS, sync, encrypted local storage |
| `crates/post-api` | The local API shared by the CLI and the daemon |
| `crates/post-cli` | `post`, the command-line client and JSON-RPC daemon |
| `crates/post-ffi` | The bridge between the app and the client core |
| `app/` | The desktop app (Flutter) |
| `scripts/` | The development server and test accounts |
| `deploy/` | Container image, Podman Quadlet and compose files, Caddyfile |
| `spikes/mls-pq` | Spike 1: OpenMLS post-quantum suite measurements |
