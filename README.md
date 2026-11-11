# Post

A small, private, end-to-end encrypted messenger for an invite-only community of
about 100 people. Post-quantum key agreement throughout, and a server that
cannot read messages or learn who talks to whom.

The design is in [`DESIGN-v2.md`](DESIGN-v2.md); section 17 tracks progress.
The original sketch is kept in [`DESIGN.md`](DESIGN.md).

## Status

Phases 1 to 4 are done: identity, the server with metadata protection, the
client core, the `post` command-line client with a JSON-RPC daemon for bots,
multiple devices per account, and group chats. DMs and groups with unlimited
edits (every version kept), deletes, profiles, admins, leaving, weekly key
refresh, device linking and removal, nicknames synced between your own devices,
typing indicators and presence work end to end. The Flutter app, attachments,
push and calls are later phases.

## Layout

| Path | What |
|---|---|
| `crates/post-identity` | Hybrid Ed25519 + ML-DSA-65 signatures, device chains, safety numbers |
| `crates/post-proto` | Client-server wire protocol |
| `crates/post-mls` | MLS settings shared by server and clients (hybrid ML-KEM-768 + X25519 suite) |
| `crates/post-server` | `postd`, the server |
| `crates/post-core` | The client core: keys, MLS, sync, encrypted local storage |
| `crates/post-api` | The local API shared by the CLI and the daemon |
| `crates/post-cli` | `post`, the command-line client and JSON-RPC daemon |
| `deploy/` | Container image, Podman Quadlet and compose files, Caddyfile |
| `spikes/mls-pq` | Spike 1: OpenMLS post-quantum suite measurements |

## Build and test

Needs Rust 1.88 or later, a C compiler, CMake, and OpenSSL development headers
(for SQLCipher).

```sh
cargo test --workspace --release
cargo clippy --workspace --all-targets
```

## Try it locally

```sh
cargo build --release
B=target/release
$B/postd keygen server.key && $B/post keygen alice.key && $B/post keygen bob.key
POST_DB=server.db POST_DB_KEY_FILE=server.key \
  $B/postd serve --domain 127.0.0.1:8080 --listen 127.0.0.1:8080 &
INVITE_A=$(POST_DB=server.db POST_DB_KEY_FILE=server.key $B/postd invite)
INVITE_B=$(POST_DB=server.db POST_DB_KEY_FILE=server.key $B/postd invite)

$B/post --profile alice --db-key-file alice.key register \
  --server ws://127.0.0.1:8080/v1/ws --invite "$INVITE_A" --username alice
$B/post --profile bob --db-key-file bob.key register \
  --server ws://127.0.0.1:8080/v1/ws --invite "$INVITE_B" --username bob

$B/post --profile alice --db-key-file alice.key send --to bob "hello"
$B/post --profile bob --db-key-file bob.key sync
$B/post --profile bob --db-key-file bob.key conversations
```

To add a second device to Alice's account, run this on the new device. It prints a
code and waits:

```sh
$B/post keygen phone.key
$B/post --profile phone --db-key-file phone.key link new \
  --server ws://127.0.0.1:8080/v1/ws --name phone
```

Then enter the code on Alice's existing device:

```sh
$B/post --profile alice --db-key-file alice.key link add XXXXX-XXXXX-XXXXX-XXXXX-XXXXX
$B/post --profile alice --db-key-file alice.key devices
```

`ws://` is for local testing only; real servers use `wss://` behind Caddy. See
[`deploy/README.md`](deploy/README.md).

## Bots

`post daemon --stdio` (or `--socket PATH`) serves the same methods as the CLI
over JSON-RPC 2.0, one JSON object per line, with live `event` notifications after
`subscribe`. `post schema` prints the OpenRPC description.
