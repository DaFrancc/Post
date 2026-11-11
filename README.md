# Postern

A small, private, end-to-end encrypted messenger for an invite-only community of
about 100 people. Post-quantum key agreement throughout, and a server that
cannot read messages or learn who talks to whom.

The design is in [`DESIGN-v2.md`](DESIGN-v2.md); section 17 tracks progress.
The original sketch is kept in [`DESIGN.md`](DESIGN.md).

## Status

Phases 1 to 3 are done: identity, the server with metadata protection, the
client core, the `postern` command-line client with a JSON-RPC daemon for bots,
and multiple devices per account. DMs with unlimited edits (every version kept),
deletes, profiles, device linking and removal, and nicknames synced between your
own devices work end to end. Groups, the Flutter app, attachments, push and calls
are later phases.

## Layout

| Path | What |
|---|---|
| `crates/postern-identity` | Hybrid Ed25519 + ML-DSA-65 signatures, device chains, safety numbers |
| `crates/postern-proto` | Client-server wire protocol |
| `crates/postern-mls` | MLS settings shared by server and clients (hybrid ML-KEM-768 + X25519 suite) |
| `crates/postern-server` | `posternd`, the server |
| `crates/postern-core` | The client core: keys, MLS, sync, encrypted local storage |
| `crates/postern-api` | The local API shared by the CLI and the daemon |
| `crates/postern-cli` | `postern`, the command-line client and JSON-RPC daemon |
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
$B/posternd keygen server.key && $B/postern keygen alice.key && $B/postern keygen bob.key
POSTERN_DB=server.db POSTERN_DB_KEY_FILE=server.key \
  $B/posternd serve --domain 127.0.0.1:8080 --listen 127.0.0.1:8080 &
INVITE_A=$(POSTERN_DB=server.db POSTERN_DB_KEY_FILE=server.key $B/posternd invite)
INVITE_B=$(POSTERN_DB=server.db POSTERN_DB_KEY_FILE=server.key $B/posternd invite)

$B/postern --profile alice --db-key-file alice.key register \
  --server ws://127.0.0.1:8080/v1/ws --invite "$INVITE_A" --username alice
$B/postern --profile bob --db-key-file bob.key register \
  --server ws://127.0.0.1:8080/v1/ws --invite "$INVITE_B" --username bob

$B/postern --profile alice --db-key-file alice.key send --to bob "hello"
$B/postern --profile bob --db-key-file bob.key sync
$B/postern --profile bob --db-key-file bob.key conversations
```

To add a second device to Alice's account, run this on the new device. It prints a
code and waits:

```sh
$B/postern keygen phone.key
$B/postern --profile phone --db-key-file phone.key link new \
  --server ws://127.0.0.1:8080/v1/ws --name phone
```

Then enter the code on Alice's existing device:

```sh
$B/postern --profile alice --db-key-file alice.key link add XXXXX-XXXXX-XXXXX-XXXXX-XXXXX
$B/postern --profile alice --db-key-file alice.key devices
```

`ws://` is for local testing only; real servers use `wss://` behind Caddy. See
[`deploy/README.md`](deploy/README.md).

## Bots

`postern daemon --stdio` (or `--socket PATH`) serves the same methods as the CLI
over JSON-RPC 2.0, one JSON object per line, with live `event` notifications after
`subscribe`. `postern schema` prints the OpenRPC description.
