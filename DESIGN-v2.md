# Post: Design

A small, private, end-to-end encrypted messenger with post-quantum key agreement:
DMs, groups, files, and voice/video calls for an invite-only community of about 100 people.

Status: design, nothing built. Supersedes `DESIGN.md` (kept unchanged for reference).
Named Post (formerly Postern): plain and hard to search for, which suits a
private, invite-only messenger.

## 1. Summary

| Layer | Choice |
|---|---|
| Messaging protocol | MLS (RFC 9420) via OpenMLS, hybrid ML-KEM-768 + X25519 ciphersuite |
| Identity | One account per person; a signed, hash-chained log of that account's devices |
| Server | Rust (axum, tokio, SQLite). Stores ciphertext, orders it, never holds a key |
| Client core | Rust (OpenMLS, SQLCipher). All crypto, protocol state and storage |
| Client UI | Flutter over the core via `flutter_rust_bridge`. Linux, Windows, Android first |
| Calls | Self-hosted LiveKit (SFU + TURN); frame encryption keyed from the MLS group |
| Push | Android only, wake-only: FCM or UnifiedPush |

### What changed from the first design, and why

| First design | This design | Reason |
|---|---|---|
| Hand-written PQXDH + ratchet, sender keys later | MLS via OpenMLS | The protocol composition is where bugs live. MLS is a standard with an audited implementation, and one mechanism covers DMs, groups and multi-device. |
| Symmetric ratchet first, "full ratchet with ML-KEM" as a late phase | Post-compromise security from day one | Every MLS commit refreshes keys with a fresh hybrid KEM. No wire-protocol change later. |
| Identity private key copied to every device | Per-device keys, signed device chain | A stolen device is removed from the chain. The identity and its safety number survive. |
| Each message encrypted ~200 times for a big group | Encrypted once, stored once | Server keeps one log row per message regardless of group size. |
| Hand-rolled SFrame, mesh calls, `pion` on the client | LiveKit SFU and SDK | Capture, codecs, echo cancellation and jitter handling are not worth writing. |
| Call key from a 1:1 session key | Call key from the MLS exporter | Works the same for group calls and multi-device, and rekeys when membership changes. |
| Go everywhere, three UIs | Rust core and server, one Flutter UI | See section 4. |

Kept from the first design: hybrid classical + post-quantum everywhere, a server that
is only a relay and storage box, chunked file encryption, content-free push, and the
security rules (now section 14).

## 2. Scope

- About 100 accounts, invite-only, one server.
- One account per person, several devices each (phone and desktops).
- 1:1 DMs and groups, up to one group containing everyone.
- Attachments, around 100 uploads a day.
- Voice and video calls, a few concurrent.
- Platforms: Linux, Windows, Android. macOS and iOS later; the design must not block them.

### Goal: the server does not learn who talks to whom

Modelled on Signal. The server stores only what it needs to run accounts: usernames,
public device chains, when an account registered, and the day each device last
connected. It never stores who is in which conversation, who sent what, or whom
anyone looked up. Section 8, "Metadata protection", says how.

### Non-goals

- Hiding traffic patterns from an operator watching the server live. While a device
  is connected, the server sees its IP address and timing, and at the moment a
  Welcome or wake is delivered it knows which device it is for. A determined
  operator could correlate these in real time. The guarantee is that none of it is
  stored.
- Federation.
- Cryptographic deniability. MLS messages are signed, so a member running a modified
  client that saves signatures as they arrive could later prove who wrote a message.
  The stock client never keeps them (section 14), and they cannot be recovered
  afterwards: the server copy is deleted after delivery and the keys that open it are
  deleted after use. This is a deliberate trade for MLS. The alternative considered
  was the Signal protocol via `libsignal`, which is deniable but AGPL, unsupported
  for outside use, and leaves group membership and multi-device to us.
- Protecting a device that is already compromised while it is unlocked.
- A web client. A server-delivered web app would let a compromised server ship
  malicious code, which defeats the threat model.

## 3. Threat model

| Adversary | Cannot | Can |
|---|---|---|
| Network observer, including one recording traffic for a future quantum computer | Read content or metadata: TLS uses hybrid key exchange, and content is under MLS hybrid keys inside it | See that a client talks to the server, and traffic volume |
| Compromised or malicious server | Read messages, files, calls or group names. Add a device to an account or a member to a group without detection. Recover the social graph from its stored data | Watch live traffic and correlate IP addresses and timing. Drop, delay or withhold messages (denial of service). Give a wrong account for a username on first contact, until the safety number is checked |
| Someone who obtains the server's disk or backups | Read messages, or learn who talks to whom | Read usernames, public device chains and registration dates, if they also have the database key |
| Stolen or compromised device | Read messages sent after it is removed. Read messages from before the keys it still holds (forward secrecy) | Read what is stored on it. Act as that user until removed |
| Malicious group member | Read conversations they are not in. Forge another member's messages | Leak what they receive. Disrupt their own groups |
| Future quantum attacker, active | Forge identity chain entries (hybrid signatures) | Forge MLS leaf signatures (Ed25519) once such a machine exists; see "Signature agility" in section 6 |

### IP addresses

Users never connect to each other, so they never learn each other's IP address:

- Messages, attachments and shared history go through `postd`.
- Every call, including 1:1, goes through the LiveKit SFU. Clients offer no host or
  peer-reflexive ICE candidates to other participants; media flows only between each
  client and the SFU.
- Link previews are built by the sender's client and sent inside the encrypted
  message. Recipients' clients never fetch a link on their own, since that would
  reveal their address to the linked site.

The server operator does see addresses, because every connection ends at the
server. To keep that exposure short-lived:

- `postd` never logs IP addresses.
- The reference Caddy config disables access logging. LiveKit logs at a level that
  omits client addresses.

Not covered: the server operator can still see addresses live, and a person's ISP
sees that they connect to the server. Hiding one's address from the server too
requires Tor or a VPN, which works for messaging but makes calls poor.

## 4. Architecture

```
            one device
 ┌─────────────────────────────────┐
 │ Flutter UI (Dart)               │
 │   └ livekit_client ─────────────┼── WebRTC, E2EE frames ──► livekit-server
 │ flutter_rust_bridge             │                            (SFU + TURN)
 │ post-core (Rust)             │                                 ▲
 │   OpenMLS · SQLCipher · sync    │                                 │ join token
 └───────────────┬─────────────────┘                                 │
                 │ TLS 1.3, hybrid key exchange                      │
                 │ WebSocket (RPC + push) and HTTPS (blobs)          │
                 ▼                                                   │
            Caddy ──► postd (Rust) ───────────────────────────────┘
                        SQLite · blob directory · push sender
```

### Language decisions

**Client core in Rust.** Go's standard library now has every primitive needed, so
primitives are not the issue. The protocol library is: OpenMLS (Rust) is audited,
maintained and ships the hybrid ciphersuites, while the Go MLS option describes itself
as work in progress, is unaudited, and has only the classical suites.

**Server in Rust.** The core is already Rust, so the server shares its wire types and
identity verification, and the real server and real cores run together in one
`cargo test`. The server also uses OpenMLS to read the public header of each message
instead of trusting client-declared values. Cost: slower builds than Go.

**UI in Flutter.** One codebase for Linux, Windows and Android, with macOS and iOS
later. LiveKit's Flutter SDK covers all of them and exposes the per-participant key
API that calls need. Tauri was ruled out because WebRTC is still disabled upstream in
WebKitGTK, which breaks calls on Linux.

### Repository layout

```
/crates/post-proto      wire types (protobuf via prost), shared by server and core
/crates/post-identity   device chain, hybrid signatures, safety numbers
/crates/post-core       MLS, sync engine, storage, attachments, call keys
/crates/post-server     postd
/crates/post-api        method and event definitions shared by the CLI and daemon; OpenRPC schema
/crates/post-cli        `post`: supported command-line client, JSON-RPC daemon for bots and scripts
/app                       Flutter app; /app/rust is the flutter_rust_bridge shim
/deploy                    Caddyfile, systemd units, LiveKit config
/docs
```

## 5. Identity

### Accounts and devices

- An **account** is a person: one username, one safety number, one member entry in a group.
- A **device** is a phone or desktop. Each generates its own keys, which never leave it.
- The account is defined by its **device chain**: a hash-chained, signed log saying
  which devices belong to it. There is no account-level private key.

Device keys:

| Key | Algorithm | Use |
|---|---|---|
| Chain key | Ed25519 + ML-DSA-65 | Signs chain links and server login challenges |
| MLS signature key | Ed25519 | Signs this device's MLS leaf and KeyPackages |
| MLS HPKE keys | ML-KEM-768 + X25519 | Managed and rotated by OpenMLS |

### Device chain

Implemented in `crates/post-identity`.

```
Link {
  version           // 1
  account_id        // absent in the genesis link
  seq               // 0, 1, 2, ...
  prev_hash         // link hash of the previous link; absent in genesis
  timestamp_ms      // informational only; clocks are not trusted
  signer            // device_id of the signing device
  op                // see below
}
SignedLink { link_bytes, sig_ed25519, sig_mldsa65, cosig_ed25519?, cosig_mldsa65? }

DeviceEntry { chain_pub_ed25519, chain_pub_mldsa65, mls_sig_pub, bot }

link_hash(b) = SHA-256(label("post/link-hash/v1") ‖ b)
device_id    = SHA-256(label("post/device-id/v1") ‖ chain_pub_ed25519 ‖ chain_pub_mldsa65)[..16]
label(s)     = len(s) as one byte ‖ s
```

A device ID is derived from its chain keys rather than chosen, so IDs cannot collide
or be claimed by another key. Encoding is protobuf; decoders require the canonical
encoding (re-encoding must reproduce the input), so a signed link has exactly one
valid form and carries no unknown fields.

The chain is public: the server stores it and every device downloads it with the
directory. So it holds only what verification needs, keys and the bot flag. Device
names ("Pixel 9") live in the account's encrypted self group, visible only to the
account's own devices; contacts see "Alice added a new device".

| Op | Signed by | Notes |
|---|---|---|
| `Genesis { server_domain, username, device }` | The first device | `account_id = link_hash(genesis link bytes)`, so the ID commits to the server, username and first device |
| `AddDevice { device }` | Any active device | Countersigned by the new device under `"post/chain-cosign/v1"`, proving it holds its keys |
| `RemoveDevice { device_id }` | Any active device | A device may remove itself, but the last device cannot be removed |
| `RotateMlsKey { device_id, mls_sig_pub }` | That device only | Rare |

Rules, enforced by every client and by the server:

- A link is valid only if **both** signatures verify, over
  `label("post/chain/v1") ‖ link_bytes`. Signatures cover the exact bytes
  received; nothing is re-serialised before verifying. ML-DSA signing is hedged
  (randomised), so the same link can carry different valid signatures; chains are
  compared by link hash, not by signature bytes.
- `seq` and `prev_hash` must extend the chain the client already holds. A client
  never accepts a chain that is shorter than, or forks from, what it has seen.
- The signer must be a device that is present and not removed at that point.
- A device ID or MLS key never appears twice in a chain, including those of removed
  devices and keys rotated away from. A returning device generates new keys.
- At most 16 devices are active at once.
- An account is a bot account if its genesis device is flagged `bot`, and every
  later device must match: bot accounts add only bot devices, other accounts never
  add one. Bot status is therefore fixed for the account's lifetime (section 11).
- Ed25519 keys must be valid points of large order. Usernames are 3 to 32
  characters of `a-z`, `0-9` and `_`, starting with a letter; server domains are
  lowercase DNS names with an optional port.
- A rejected link leaves the client's chain unchanged.

An `AddDevice` link is about 9 KB (the ML-DSA key and two signatures). Chains are
short and cached. Every device downloads the whole directory, all accounts' chains,
and then only what changed, so the server never learns whose chain anyone is
interested in. At 100 accounts the first download is about 1 MB.

**Why hybrid signatures here but not in MLS.** Checking a safety number is the one
step a human does by hand, so its anchor should not need redoing when quantum
computers arrive. Chain links are rare, so 3.3 KB signatures cost nothing. Because
both signatures must verify, a flaw in a young ML-DSA library cannot make the chain
weaker than Ed25519 alone.

### Protection against a withholding server

The server cannot forge a link, but it could hide one, such as the removal of a
stolen phone. Two defences:

- **Chain-head gossip.** Every message carries, inside the encryption, the sender's
  current chain `seq` and head hash. A recipient whose copy is behind fetches the
  missing links; if the server cannot produce them, the client raises a security
  warning.
- **Own-chain check.** Each device compares the server's copy of its own chain with
  its local copy on every connect.

### Linking a device

Implemented in `crates/post-core/src/link.rs`, with the server side in
`crates/post-server/src/link.rs`.

The new device generates its keys and a random **link code** of 125 bits, shown as 25
characters of Crockford base32 (`A884A-1S8XG-M86GD-BWP2J-AM8TG`), or as a QR code in
the app. Case, spaces and dashes do not matter, and O, I and L read as 0, 1 and 1.
Both devices derive two things from the code with HKDF-SHA256:

- a **rendezvous ID**, naming a set of write-once slots on the server;
- a **channel key**, sealing every slot with XChaCha20-Poly1305, bound to the
  rendezvous and slot number so sealed payloads cannot be moved between slots.

The server sees only the rendezvous ID and ciphertext. It cannot read or forge a
slot, and recovering the code from the rendezvous ID means searching 2^125 codes.
Rendezvous live in server memory only, for ten minutes, with limits per connection
and in total. Both devices use anonymous connections: the new device has no account
yet, and the slots carry nothing the server should tie to a session.

| Slot | Written by | Holds |
|---|---|---|
| 0 `OFFER` | new device | its `DeviceEntry` and the name the user gave it |
| 1 `ANSWER` | existing device | the unsigned `AddDevice` link, the username, the number of conversations |
| 2 `COSIGN` | new device | its countersignature over that link |
| 3 `DONE` | existing device | the server accepted the link |
| 4 `READY` | new device | it has logged in and uploaded KeyPackages |
| 5 `ADDED` | existing device | it added the new device to its conversations |

1. The user enters the code on an existing device, which reads the offer and shows
   "Add the device *Pixel 9* to your account?". It refuses a device already in the
   chain, or one whose bot flag does not match the account.
2. On approval it sends the `AddDevice` link bytes. The new device countersigns only
   a link that adds exactly its own entry, and saves its keys before the
   countersignature leaves, since the link may land whatever happens next.
3. The existing device signs, verifies the whole link against its chain, records it,
   and uploads it. The record is kept until the server answers, so a lost reply is
   settled at the next login: done if the server's chain has the link, uploaded again
   if the chain can still take it, dropped with a warning if another link took its
   place. It then sends the device's name to the self group.
4. The new device logs in and uploads enough KeyPackages for every conversation it
   is about to join (it was told how many), then signals `READY`.
5. The existing device adds it to the self group and every conversation (section 6,
   "Adding a new device to its conversations").

No private key moves between devices at any point. The new device takes its username
from the verified chain once it logs in, not from what it was told.

### Removing a device

Any device publishes `RemoveDevice`. Two things follow:

- The server disconnects the device, refuses its session logins and deletes its
  mailbox and KeyPackages at once.
- The removing device commits its removal from every MLS group. Any other member who
  sees the chain link may do the same, so removal does not depend on one device.
  Until that commit, the removed device still holds the current epoch's access key,
  so it can fetch that epoch's ciphertext; the commit changes the key.
- Contacts learn of the removal from the chain head in the account's next message
  (below), or at their next directory fetch.

A device cannot remove itself yet; another device of the account removes it. Signing
out a device from itself, which would also wipe its local data, comes with the app.

### Verification

- The safety number for two accounts is derived from
  `SHA-512(label("post/safety/v1") ‖ min(id_a, id_b) ‖ max(id_a, id_b))`: each
  5-byte chunk of the first 60 bytes, modulo 100 000, gives one 5-digit group, for
  60 digits in 12 groups. Also shown as a QR code. (SHA-512 because 60 digits need
  60 bytes of hash output.)
- Because account IDs do not change when devices do, a verified contact stays
  verified through new phones and removed laptops. Device changes show a quiet notice.
- Clients pin `username → account_id` on first sight. A change means the account was
  reset: verification is voided and a blocking warning is shown before any send.

### Losing every device

An admin runs `postd admin reset-account <user>`, which issues a one-time code.
The user registers a new genesis under the same username, and every contact sees the
reset warning. A later phase adds an optional 24-word recovery key that sits in the
chain as an offline device able to sign `AddDevice`.

## 6. Conversations (MLS)

### Mapping

- Every conversation is one MLS group. A DM is a group of two accounts.
- Each **device** is one leaf. Members at the application level are accounts.
- Invariant: a group's leaves are exactly the current devices of its member accounts.
- Each account also has a **self group** of only its own devices. It carries the
  account's profile, private nicknames, device names, verified contacts and read
  positions now; pinned usernames, settings and history-transfer keys later. The account's
  oldest device creates it; the others wait to be added, so an account does not end
  up with one per device. If two ever exist, the lower group ID wins. It is never
  listed as a conversation, takes no messages, and clients refuse any Welcome or
  commit that would add another account or change its metadata.
- Values in the self group (`SelfSync` messages) follow its log order: a value from
  the log replaces one set at an earlier position; a change this device has sent but
  the server has not placed yet is never replaced, since it will land after
  everything already logged; clearing leaves a marker so an older value cannot
  return. A device joining the self group gets a snapshot that fills in only values
  it lacks, so a stale snapshot never undoes a newer change.
- A read position is a position in a group's server log, the same on every device;
  later messages from others are unread. Positions only move forward, so they merge
  by taking the larger, in any order. A device sends them only when the account has
  another device to read them.

Your other devices are ordinary members of each conversation, so they receive what
you send with no special self-sync path.

### Ciphersuite

`MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`: HPKE with the ML-KEM-768 + X25519
hybrid KEM, AES-256-GCM, SHA-384, Ed25519 leaf signatures.

- OpenMLS 0.9 exposes it behind the `draft-ietf-mls-pq-ciphersuites` feature. Its KEM
  is X-Wing (draft 06).
- Provider: `openmls_libcrux_crypto`, whose ML-KEM is formally verified. That provider
  accepts the suite but does not advertise it, so upstream does not test the
  combination. Spike 1 showed a libcrux device and a RustCrypto device interoperate
  in both directions; that interop test moves into `post-core` so an upgrade that
  breaks agreement fails CI. See `spikes/mls-pq/RESULTS.md`.
- The IETF draft (revision 07, submitted for publication November 2026) assigns it
  `0x0011`; OpenMLS uses the provisional `0x004E`. Post clients only talk to each
  other, so this is not an interoperability problem. Pin the OpenMLS version and
  budget one group re-initialisation when the RFC is published.

**Signature agility.** Leaf signatures are classical. Forging one needs a quantum
computer at the moment of the attack, not a recording, so this is not urgent. When
the ML-DSA suites in the same draft mature, groups move to one by re-initialisation;
the identity layer does not change.

### Credentials and the membership check

A leaf's credential is a basic credential holding `account_id ‖ device_id`. Before
merging any commit, and when joining from a Welcome, the client checks every leaf:

1. The credential resolves to a device that is present and not removed in that
   account's verified chain.
2. The leaf's signature key equals that device's `mls_sig_pub`.
3. If the cached chain lacks the device, fetch newer links and retry. If it still
   fails, reject the commit and raise a security warning.

This check is what stops the server, or anyone, slipping a device into a group.

### Group policy

MLS lets any member commit anything, so policy is enforced by every client when it
inspects a staged commit:

- Group name, avatar reference, admin list, conversation type and history setting
  live in a GroupContext extension (`post-mls::GroupMeta`, type `0xff10`). They
  are part of the agreed group state and the server never sees them. Clients decode
  it strictly and reject unknown fields, so a group using a newer format is refused
  by older clients ("update the app") instead of being half-understood.
- Only an admin may add or remove an **account**, or change the extension. In a DM
  both sides are admins and the account set is fixed.
- Any member may add a **device** of an account that is already a member, and may
  remove a device whose removal appears in its account's chain.
- A commit that changes the extension must set exactly Post's extensions, may not
  change the conversation kind, and must leave at least one admin among the members.
  A self group never changes its metadata or gains another account.
- Removing an account takes all its leaves and its admin role. Removals are justified
  only by facts that do not change with time: the committer's own account, the
  member's own leave proposal, removal from the chain, or an admin's decision. (A
  device judging its own past removal by today's directory must reach the same
  answer as when it was committed.)
- Leaving: a member cannot commit its own removal, so the leaving device sends a
  remove proposal for each of its account's leaves, and the next member to sync
  commits them. Proposals are only valid in their epoch, and a commit from a member
  that had not yet seen them would drop them, so the leaving device proposes again in
  each new epoch until its removal lands. It shows the group as left at once and
  keeps nothing more from it. The only admin hands the group to the longest-standing
  member first. Clients accept a proposal only if it removes the proposer's own
  devices, before it can reach the proposal store and ride along in an honest
  member's commit.

A commit that breaks policy is rejected locally and reported to the user.

**Being added back.** A Welcome for a group a device already knows replaces that
group's state only if the device is no longer in it (removed, or left); the
conversation and its messages carry on. A group the device is still in is never
replaced, or anyone holding one of its KeyPackages could knock it out of a
conversation with a forged Welcome reusing the group's ID. Sync reads group logs
before Welcomes, so a device removed and added back while offline sees its removal
first. A fetch refused for a wrong access key means the device fell behind the
server's 30-day log; it stops following that group, with a notice, so being added
back works there too.

### Ordering

The server keeps one append-only log per group and enforces a single rule: **a
message is accepted only if its epoch equals the group's current epoch.**

```
Commit(group, access_key, next_access_hash, client_msg_id, bytes, welcome, welcome_to[])
  in one transaction:
    (group, client_msg_id) already logged  →  return its seq (idempotent retry)
    SHA-256(access_key) != hash for the group's current epoch  →  reject
    epoch in the MLS header != groups.epoch  →  reject with the current epoch
    append to the log; groups.epoch += 1; store next_access_hash for the new epoch;
    put the Welcome in each welcome_to mailbox

Send(group, access_key, client_msg_id, bytes)
    same idempotency, access and epoch checks; append to the log
```

The committer computes the next epoch's access key from its staged commit before
merging it. A member could commit a wrong `next_access_hash` and lock the group,
but a member can disrupt its own group in other ways too.

Client side:

- A commit stays pending in OpenMLS until the server accepts it, then is merged. On
  rejection it is discarded, the intervening log entries are processed, and it is
  rebuilt.
- A rejected application message is re-encrypted in the new epoch and resent with
  the same `client_msg_id`.
- Devices process each log strictly in sequence. Because of the epoch rule they never
  need keys from a past epoch, so past-epoch retention is set to zero.
- A device cannot decrypt its own MLS messages. It stores the plaintext when it
  sends, and recognises its own entries in the log by `client_msg_id`, which it
  generated. The log records no sender.

Commits are sent as encrypted MLS messages, and the server keeps no membership, so
it neither sees nor tracks who was added or removed. The only roster information in
a request is `welcome_to`, the devices whose mailboxes get the Welcome.

### KeyPackages

Each device uploads a batch of single-use KeyPackages and one last-resort package.
The server hands out one per claim and deletes it; the last-resort package is used
only when the batch is empty. Devices top up when the server reports a low count.

### Adding a new device to its conversations

Group membership follows the device chains. On every sync, each device compares each
of its groups with the verified chains and commits what differs:

- this account's devices missing from the group are added;
- leaves of devices their accounts have removed are removed.

Any member may do either, so a change does not depend on one device. If two devices
race, the server takes one commit per epoch and the other finds nothing left to do.
Only an account's own devices add its devices, so other members never spend tokens
on them.

For each device to add, one `ClaimKeyPackages` names that device and a count, for
one token, and returns up to that many one-time KeyPackages plus the last-resort one
when they run short; the last-resort package is then used for the remaining groups.
A device with no KeyPackages yet is retried after ten minutes rather than on every
sync.

A commit is recorded before it is sent and merged only when it comes back through
the group's log, so messages logged before it in the same epoch are still read with
the old keys. A commit whose outcome is unknown is retried at login (the server
answers a retry with the existing entry), and one that lost its epoch to another
commit is dropped.

Right after linking, the existing device runs this at once, so the new device joins
every conversation within seconds. For someone in 150 conversations that is 150
small commits; DMs are a few kilobytes each.

The new device gets history the same way as a new member (below), week by week on
request, answered by the account's other devices. A person's own devices may go
back through all of their history, whatever the group's setting.

### History for new members

MLS gives a newcomer only what is sent after they join. Each group has a setting,
`GroupMeta.history`, that admins control:

- `Hidden`: newcomers see only new messages.
- `Shared` with `history_days` (0 = everything): the member who adds someone shares
  past messages with them.

History moves a week at a time, and only when someone wants it. The server cannot
produce history itself: it never has plaintext, and its ciphertext copies are
deleted once delivered. It relays requests and stores the encrypted weekly archives.

Each week is one archive, built and shipped like an attachment (section 9): the
conversation's messages in that week as a protobuf list of `{message id, sender
account, sent at, body}`, encrypted under a fresh random key with the attachment
format and uploaded as a blob.

1. **On add.** Right after its add commit is accepted, the adding device uploads the
   most recent week and sends, as an ordinary message in the new epoch:
   `HistoryShared { request_id, blob_id, key, sha256, count, from, to, earliest }`,
   where `earliest` is the oldest date the group's setting allows sharing back to.
2. **On scroll.** When the newcomer scrolls past what it has, its app sends
   `HistoryRequest { request_id, before, responder }` into the group, addressed to
   the device that shared the previous week. Only the addressed device answers, with
   the next week back as another `HistoryShared`, so there is one upload per request.
3. **If the responder is offline**, the request waits in the group log and is
   answered when that device processes it. After a timeout (default 10 minutes) with
   no answer, the newcomer re-sends the request with no responder named. Any member
   may then answer; a device skips a request already answered in the log.
4. The newcomer downloads each archive, verifies its hash, decrypts and imports it.
   Imported messages are marked with who shared them.

Properties:

- Bandwidth and storage match what is actually read. Nobody uploads years of history
  up front.
- Responders never share before `earliest`, which follows the group's
  `history_days`. A request outside the window gets no answer, and the app shows
  "history before this date is not shared in this group".
- The server stores only ciphertext and cannot tell a history blob from a photo.
- Every member sees the `HistoryRequest` and `HistoryShared` messages, so sharing is
  visible.
- **Imported history is only as trustworthy as its sharer.** Clients keep no message
  signatures (the deniability rule in section 14), so old messages cannot be
  verified one by one. The app labels them "shared by Bob" and never presents them
  as individually verified. A member could fabricate history; other members who see
  the share can dispute it.
- A busy week above the size limit is split into several blobs.
- The same mechanism serves a person's newly linked device, whose own other
  devices answer its requests.
- Ships with attachments (phase 6), which provide the blob format and upload path.

### Future: community servers

Discord-style servers (one community with many channels, roles and voice channels)
are not built now, but nothing above blocks them. The intended shape:

- A **space** is one MLS group holding the community's membership, roles and channel
  list in its metadata.
- Each **channel** is its own MLS group, with history set to `Shared` so newcomers
  can scroll back. Joining the space means being added to its channels.
- Voice channels are LiveKit rooms keyed from the channel's group (section 10).
- New `ConversationKind` values and `GroupMeta` fields carry this. Because metadata
  decoding is strict, older clients refuse those groups rather than misread them.
- What needs design work then: role-based permissions in place of the simple admin
  list, and keeping hundreds of channel groups in step when someone joins or leaves.

### Key refresh

A device commits an update of its own leaf in each group when that leaf is a week
old. Each group falls due up to a day early, by a spread derived from its ID, so a
device's refreshes do not all land at once. Any own commit with a path counts. This
is what gives post-compromise security, and each refresh uses fresh hybrid KEM keys.
`refresh_keys` makes every leaf due at once, for after a device may have been
exposed.

### Stale devices

- A device unseen for 30 days is flagged stale by the server's hourly job and listed
  in the directory; logging in clears the flag. Clients fetch the directory at the
  start of every sync (one small request when nothing changed).
- Its account's other devices, or a group admin, remove its leaves wherever the
  account keeps another, non-stale device. An account's last leaf is never removed,
  so an account stays a member while all its devices are away.
- The server keeps group logs for 30 days. A device that falls behind that horizon
  stops following the group (above, "Being added back").
- A returning device is added back by its account's other devices, using its
  KeyPackages on the server, and rejoins after seeing its removals.

### Ephemeral signals

Typing indicators and presence are not sent as MLS application messages: undelivered
ones would leave gaps in the sender's ratchet that offline devices would have to
skip, and they would fill the group log. Instead:

- The payload is `{ sender account, sender device, kind, body, sent_at }`, signed with
  the sender device's MLS signature key (Ed25519) under
  `label("post/ephemeral/v1") ‖ group_id ‖ epoch`.
- The whole thing is sealed with XChaCha20-Poly1305 under
  `MLS-Exporter("post/ephemeral/v1", "", 32)` with a random nonce.
- It is sent on the anonymous connection with the group's current access key
  (`SendEphemeral`). The server relays it to connections currently subscribed to the
  group and stores nothing.
- Recipients check that the signing key belongs to the claimed device's leaf in the
  group, so a member cannot fake another member's typing or status. The signature is
  checked and then discarded (section 14).
- Signals more than 60 seconds old, or from more than 60 seconds in the future, on
  arrival are dropped.

Implemented in `crates/post-core/src/ephemeral.rs`. A device sends at most 15
signals a second when announcing presence to many groups, under the server's relay
limit of 20.

## 7. Profiles, presence and message content

Everything here is end-to-end encrypted. The server never stores or sees a profile,
a status, a typing signal or a message.

### Profile

```proto
message Profile {
  uint64 version = 1;          // increases with every change; the highest wins
  string display_name = 2;     // up to 64 characters; empty means show the username
  Avatar avatar = 3;           // blob reference and key (phase 6)
  string typing_template = 4;  // see below; empty means the standard text
  Note   note = 5;
  string arrival = 6;          // how their messages arrive on screen; see below
}
message Note { string text = 1; uint64 set_at_ms = 2; uint64 expires_at_ms = 3; } // 0 = no expiry
```

- **Arrival style.** Each person picks how their messages arrive on everyone's
  screen: the name of an app animation preset (`bubble`, `rise`, `decode`,
  `typewriter`, `focus`, `wave`, `instant`), up to 32 lowercase letters, digits and
  dashes; empty means the app's default. An app that does not know a name uses its
  default, so new presets need no protocol change, and a received name that is not
  valid is dropped rather than refusing the profile. Viewers can turn everyone's
  styles off, and the system's reduced-motion setting turns them all off.

- **Distribution.** A profile is an ordinary message body, `Content.profile`, sent into
  every conversation the account follows whenever it changes. When a commit adds a
  new account to a group, each member's client sends its current profile into the
  group so the newcomer has it. Clients keep only the highest `version` per account.
- **Why not a server-side profile store** (as Signal has): fetching someone's profile
  by account would tell the server whom a person is interested in, and the server
  would hold encrypted profiles linked to accounts. Sending profiles through
  conversations keeps them with the people who already talk to each other. The cost,
  one small message per conversation per change, is fine at this scale.
- **Display names cannot be used to impersonate.** The client always shows the
  @username next to a display name in member lists and details, and flags a display
  name that matches another member's username or display name.

### Names

Two separate things:

- **Display name**: the name a person chooses for themselves. One global display
  name lives in their profile. They may also set a **per-conversation display name**
  that only that conversation sees, like a server nickname on Discord. It is sent as
  `Content.conversation_name { name }` into that one conversation only; an empty name
  clears the override. The latest in that group's log wins.
- **Nickname**: a name a person gives someone else, visible only to themselves. It is
  stored in the account's self group, so their own devices share it, and is never
  sent to anyone else.

What a viewer sees for someone, first match wins:

1. The viewer's own nickname for that person.
2. That person's display name for this conversation.
3. That person's global display name.
4. That person's @username.

The same order fills `$$NAME$$` in typing templates. The @username stays visible in
member lists and details whatever name is shown.
- **Avatar.** A square image, resized by the client to at most 512×512 and 200 KB,
  encrypted and uploaded as a blob like an attachment (section 9), with its key in
  the profile. Avatar blobs live 90 days, and the owner's client renews them over the
  anonymous connection with a renewal secret only it holds. Ships with attachments
  in phase 6.
- **Note.** Up to 60 characters, like an Instagram note. Expires 24 hours after it is
  set unless the user chooses no expiry; expired notes are hidden by every client.

### Typing indicators

- Sent as an ephemeral signal (section 6) while the user types, at most every 3
  seconds; the indicator disappears 6 seconds after the last one.
- **Custom template.** A user may set a template such as `the one who types is
  $$NAME$$`. Rules: up to 80 characters, `$$NAME$$` appears 1 to 3 times, no control
  characters or newlines. The viewer's client replaces each `$$NAME$$` with the name
  the viewer sees for that person in that conversation (see "Names"). Templates are
  plain text, never markup.
- With several people typing, each person's line is shown; with more than three,
  "Several people are typing".
- Each user can choose to see standard text instead of others' custom templates.

### Presence

| State | Others see | Notifications on own devices |
|---|---|---|
| Online | Online | Normal |
| Away | Away (set by hand, or automatically after 10 minutes idle) | Normal |
| Do not disturb | Do not disturb | Silenced, calls do not ring |
| Invisible | Offline | Normal |

- Sent as an ephemeral signal to every followed conversation when the state changes,
  and again every 10 minutes while the app is running. A clean shutdown sends
  "offline". If nothing arrives for 25 minutes, the person is shown offline.
- **Invisible sends nothing**, so other users see the person as offline while they
  still receive everything. The server operator can still see that the device is
  connected, as with any connection.
- Presence is held in memory only, never written to the database.
- In very large groups, presence is shown but not re-announced into groups with no
  activity for 30 days, to keep relay traffic small.

### Message content

Inside each MLS application message:

```proto
message Content {
  bytes  id = 1;              // 16 random bytes, equal to client_msg_id
  uint64 sent_at_ms = 2;
  ChainHead sender_chain = 3; // seq and head hash, for chain-head gossip
  oneof body {
    Text text = 10;  Attachment attachment = 11;  Reaction reaction = 12;
    Edit edit = 13;  Delete delete = 14;          Receipt receipt = 15;
    CallEvent call = 16;  Profile profile = 17;   ConversationName conversation_name = 18;
  }
}
```

- Unknown body types render as "unsupported message, update the app". Never dropped
  silently. Profile and name messages are not shown in the timeline.
- MLS padding rounds ciphertext to a 256-byte multiple to blur message length.
- Read receipts are sent in DMs and small groups only.

### Mentions

- A mention is `@` followed by a username in a message's text: no separate field, so
  every client (and the CLI) reads it the same way, and older clients show it as text.
- It counts only where nothing touches it (no letter, digit or `_` right before the
  `@` or right after the name), outside inline code and code blocks, and not after a
  backslash (`\@alice` is literal). `_` is part of usernames, so `_` italics cannot
  wrap a mention; `*` italics can.
- Each client works out who a message mentions from its latest text, so an edit can
  add or remove a mention, and a deleted message mentions no one.
- A conversation's unread count says how many unread messages mention the user. Apps
  show that count on groups, where ordinary unread messages show only as a dot.
- Search can narrow to messages mentioning someone (`mentions:alice`).

### Edits

- `Edit { target, body }` replaces the text of message `target`. There is no time
  limit and no limit on the number of edits.
- Only the account that sent the original may edit it. An edit from anyone else is
  rejected and shown as a security notice.
- Every client keeps **every version**: the original is version 0 and each edit adds
  the next. The app shows the latest, marked "edited"; opening it lists all versions
  with their times. All members see the same versions.
- Versions are ordered by the group log, not by senders' clocks, so every device
  agrees on the order.
- Edits of a deleted message, or of a message in another conversation, are ignored.
- Shared history (section 6) carries all versions of each message.

### Deletes

- `Delete { target }` removes message `target` and all its versions from every
  member's device, leaving a "message deleted" marker. Keeping old versions after a
  delete would defeat the delete.
- Only the account that sent the message may delete it, in DMs and groups alike.
  Admins cannot delete other people's messages. A delete from anyone else is
  rejected and shown as a security notice.
- No time limit.
- Deleted messages are left out of shared history.
- "Delete for me" removes a message from the local device only and sends nothing.
- Like every messenger's delete, this binds honest clients only. Someone running a
  modified client, or who took a screenshot, keeps what they saw.

## 8. Server

`postd` is one Rust binary: axum, tokio, and SQLite in WAL mode through `rusqlite`
with a single writer.

### Metadata protection

The server must not store who talks to whom. Three mechanisms:

1. **Two kinds of connection.** Each device keeps two WebSockets:
   - The **session**, logged in as the device. Used only for things tied to the
     device anyway: its mailbox, its KeyPackages, its own chain, the directory, and
     collecting anonymous tokens.
   - The **anonymous connection**, never logged in. Used for everything about
     conversations: sending, fetching group logs, creating groups, claiming other
     people's KeyPackages. The server cannot tell which device it belongs to except
     by live IP and timing correlation, which it does not record.
   A request sent on the wrong kind of connection is refused, so a client bug cannot
   quietly link the two.
2. **Group access keys instead of membership.** For every epoch, members derive
   `access = MLS-Exporter("post/group-access/v1", "", 32)`. The server stores only
   `SHA-256(access)` for each epoch. Sending, fetching and subscribing to a group
   prove membership by presenting that epoch's key, without saying who is asking.
   The server keeps no list of members.
3. **Anonymous tokens for the rest.** Creating a group and claiming someone's
   KeyPackages need proof of being a registered user, without saying which one.
   Over its session, a device collects RSA blind-signed tokens (RFC 9474), up to a
   daily limit. Each anonymous request of those kinds spends one token. Blinding
   hides perfectly, so even unlimited computing power cannot link a token to the
   device that collected it.

What the server learns, and for how long:

| Fact | Learned | Stored |
|---|---|---|
| A group exists, its epoch, its traffic volume and timing | Yes | As a random ID, for 30 days |
| Who is in a group | No | No |
| Who sent a message | No (MLS also encrypts the sender inside the ciphertext) | No |
| That some device fetched groups A, B and C | Live, on one anonymous connection | No |
| Which device a Welcome is for | Yes, to deliver it | Until fetched |
| Whom someone looked up | No: every device downloads the whole directory | No |
| Whose KeyPackages were claimed | That someone claimed them | Only that one fewer remains |
| Usernames, public chains, registration date, last-seen day | Yes | Yes |

**About the tokens and quantum computers.** RSA blind signatures are not
post-quantum: a large quantum computer could forge tokens and so bypass the spam
gate on group creation and KeyPackage claims. It could not read anything, pose as
anyone or deanonymise past requests, because blinding is information-theoretically
hiding. Post-quantum blind signatures have no standard or maintained library yet.
Tokens therefore carry a version so the scheme can be replaced, and the server
alerts if more tokens are spent than it issued, which is what forgery would look
like.

### Transport and login

- Protobuf request/response frames over WebSocket, plus server pushes. Blobs use
  plain HTTPS so they can resume and stream.
- TLS 1.3 with the `X25519MLKEM768` hybrid group, so recorded traffic does not expose
  metadata later. rustls offers it with the aws-lc-rs provider; the
  `prefer-post-quantum` feature puts it first. A deploy check asserts the negotiated
  group.
- Every connection starts with the server's `Hello` carrying a fresh nonce.
- Session login: the device signs `label("post/auth/v1") ‖ label(server_domain)
  ‖ nonce` with its chain key (both signatures). The socket is then bound to that
  device.
- Registration needs a single-use, expiring invite code and a valid genesis link,
  signed the same way.
- An anonymous connection sends requests straight after `Hello`.

### Requests

| Connection | Area | Requests |
|---|---|---|
| Either, before login | Accounts | `Register`, `Auth` |
| Session | Directory | `Directory(since)`: every account's username and chain links changed since a version |
| Session | Own chain | `AppendChain` |
| Session | KeyPackages | `UploadKeyPackages` |
| Session | Mailbox | `FetchMailbox`, `AckMailbox` (Welcomes addressed to this device) |
| Session | Tokens | `IssueTokens(blinded messages)` |
| Anonymous | Groups | `CreateGroup` (token), `Send` (application messages and proposals), `Commit`, `FetchGroups`, `Subscribe` (access key) |
| Anonymous | KeyPackages | `ClaimKeyPackages(account)` or `(account, device, count)` (token) |
| Anonymous | Linking | `LinkPut`, `LinkGet` (sealed rendezvous slots; section 5) |
| Session | Later phases | `Push.Register`, `Call.Token`, blob tokens |

### What the server validates

- Chain links: full verification, the same code the clients run.
- MLS messages: parses the public header with OpenMLS and takes group, epoch and
  content type from it.
- The access key presented matches the stored hash for that group and epoch.
- Tokens: valid signature under a current token key, never spent before.
- KeyPackages: signature, ciphersuite, and that the credential matches the uploader.
- Size limits and rate limits.

### Data model

```sql
accounts(account_id PK, username UNIQUE, is_bot, created_day, disabled_at)
chain_links(account_id, seq, link, PRIMARY KEY (account_id, seq))
devices(device_id PK, account_id, mls_sig_pub, active, last_seen_day)
key_packages(id PK, device_id, kp, last_resort, created_day)
mailbox(device_id, seq, welcome, created_at, PRIMARY KEY (device_id, seq))
groups(group_id PK, epoch, next_seq, created_day)
group_epochs(group_id, epoch, access_hash, PRIMARY KEY (group_id, epoch))
group_log(group_id, seq, epoch, kind, client_msg_id, payload, created_at,
          PRIMARY KEY (group_id, seq))
          -- UNIQUE (group_id, client_msg_id): retries are idempotent
token_keys(key_id PK, secret_key, public_key, created_at, retires_at)
spent_tokens(key_id, token_hash, PRIMARY KEY (key_id, token_hash))
token_issuance(device_id, day, count, PRIMARY KEY (device_id, day))
invites(code_hash PK, bot, created_at, expires_at, used_by, used_at)
directory_version(version)  -- bumped whenever an account or chain changes
```

No table links a device or account to a group. `group_log` stores no sender.
`token_issuance` counts tokens per device per day for rate limiting, which reveals
how many tokens a device took, not what it spent them on.

The database itself is encrypted at rest with SQLCipher. The key comes from a file,
a container secret or systemd credentials, never from the command line.

### Delivery

- **Group logs.** On connect, the anonymous connection sends `FetchGroups` with, for
  each group, its ID, the epoch the device is in, that epoch's access key and the
  last sequence number processed. A key for epoch E returns only entries from epoch
  E. After processing a commit, the device derives the next epoch's key and fetches
  again. So a device that was away catches up one epoch at a time, and a removed
  member cannot follow the group after its removal.
- **Wakes.** The anonymous connection subscribes to its groups with their access
  keys. The server keeps subscriptions in memory only and sends `Wake` on new
  entries. Phones that are offline get a push in phase 7: the sender's client names
  the recipient devices in its send so the server can push them, used immediately
  and not stored.
- **Mailboxes.** A Welcome is addressed to devices, so it goes to their mailboxes,
  fetched over the session and deleted when acknowledged. The Welcome is opaque:
  the group ID is inside its encryption. Only the server's transient view of the
  `CreateGroup` or `Commit` request links a group to the devices being welcomed.
- The client records its position in each group itself and advances it only after
  the resulting state is committed locally (section 11). The server keeps no
  per-device read positions.
- **One DM per pair**, client-side. The server cannot enforce it without knowing
  who is in a DM. If two people create DMs with each other at once, both clients
  keep the group with the lower ID and retire the other; messages from both stay
  visible in the one conversation.

### Administration

Admin actions run through a local CLI over a Unix socket: create invites, disable or
reset an account, set quotas, show storage. There is no admin HTTP surface.
Disabling an account disconnects its devices and refuses their logins at once;
members' clients then remove its leaves.

### Retention

- Group log entries and epochs: 30 days. The server cannot tell when everyone has
  read an entry, so retention is by time only.
- Mailbox entries: deleted when acknowledged, or after 30 days.
- Spent tokens: kept until their token key retires (weekly rotation, one week
  overlap).
- Blobs: 30 days.
- Logs record IDs and errors only. No payloads, and no per-message lines at the
  default level.

## 9. Attachments

1. Generate a random 32-byte key per file.
2. Encrypt in 64 KiB chunks with ChaCha20-Poly1305 using the STREAM construction:
   the nonce carries the chunk counter and a last-chunk flag, which prevents
   reordering and truncation. Use an existing implementation (RustCrypto
   `aead-stream`), not a custom one.
3. Upload with resumable `PATCH` requests. The server returns a random 128-bit blob ID.
4. Send `{ blob_id, key, sha256_of_ciphertext, size, mime, name, thumbnail }` as a
   normal message. Small thumbnails travel inline.
5. The receiver checks the hash, then decrypts chunk by chunk, failing on any bad
   chunk or a missing final flag.

- Any signed-in account may fetch a blob by ID. The capability is the unguessable ID
  plus the key, so the server does not learn which conversation a blob belongs to.
- Voice notes and video messages are attachments.
- Default limits: 512 MB per file, 5 GB per account per 30 days, both configurable.

## 10. Calls

### Media

- `livekit-server` runs beside `postd` as the SFU and TURN server for all calls,
  including 1:1. One path for every call, and peers never learn each other's IP.
- Clients use `livekit_client`, which handles capture, codecs, echo cancellation,
  congestion control and frame encryption.
- `Call.Token` checks that the caller's account is in the conversation and returns a
  short-lived LiveKit token for room `hex(call_id)` with identity `hex(device_id)`.

### Keys

```
call_base  = MLS-Exporter("post/call/v1", call_id, 32)          // per epoch
sender_key = HKDF-SHA256(ikm = call_base, salt = call_id,
                         info = "post/call/sender" ‖ device_id, 32)

keyProvider.setRawKey(sender_key, participantId: hex(device_id),
                      keyIndex: epoch % keyRingSize)
```

- The key provider is created with `sharedKey: false`, so each sender has its own key.
- Keys come from the MLS group, so calls inherit its post-quantum security and
  membership. Anyone in the conversation can join; nobody else can derive the keys.
- On each new epoch every participant derives the next key and installs it at the
  next index. Senders switch after a two-second grace period; old keys are dropped
  after ten seconds.
- A member removed from the conversation mid-call loses the keys at that epoch, and
  the server also ejects them from the room.

Limits to be aware of:

- Media frames are encrypted but not signed. A member colluding with the server could
  send frames attributed to another member of the same call.
- The frame cipher is whatever LiveKit's SDK implements (AES-GCM). Confirm the key
  length during the spike.

### Signalling

- `CallEvent { start | decline | end, call_id, media }` is an ordinary message.
- DMs and small groups ring. Large groups show a "join call" banner instead.
- A ringing message is marked urgent, so Android gets a high-priority push and shows
  a full-screen incoming-call notification.

## 11. Client

### Core

`post-core` owns every key, all protocol state and the database. Dart is a view.
The API is coarse commands plus one event stream:

```rust
Core::open(profile_dir, db_key)
core.register(server, invite, username)    core.link_begin() / link_approve(code)
core.events() -> Stream<Event>             // MessageAdded, ConversationUpdated,
                                           // CallIncoming, SecurityNotice, ...
core.conversations()                       core.messages(conv, before, limit)
core.send_text(conv, text, reply_to)       core.send_attachment(conv, path)
core.call_keys(conv, call_id) -> Stream<CallKey>
core.sync_once(deadline)                   // headless: used by push wake-ups
```

`sync_once` exists so a push handler can run the core with no UI. Both Dart isolates
load the same native library in one process, so they share one core instance. The
same entry point is what an iOS notification extension will need later.

### Storage and crash safety

- One SQLCipher database per profile holds OpenMLS state, cached chains,
  conversations, messages (with FTS5 for local search), the outbox and the
  attachment index.
- OpenMLS storage is implemented over the same database, so these rules can hold:
  - **Receive:** new MLS state, the decrypted message and the log position commit in
    one transaction. The acknowledgement to the server goes out after it.
  - **Send:** the ciphertext is written to the outbox, with the advanced MLS state,
    before anything is transmitted. Retries resend identical bytes.
  - **Commit:** the pending commit is persisted before sending. On restart, seeing
    its own commit in the log means merge; seeing a different one means discard.
- The database key is 32 random bytes wrapped by the platform keystore: Android
  Keystore, Windows DPAPI, or the Secret Service on Linux. Where no keyring exists,
  fall back to a passphrase stretched with Argon2id.
- Key types are wiped on drop (`zeroize`).

### Platforms

| Platform | Background delivery | Distribution |
|---|---|---|
| Android | FCM or UnifiedPush wake, then `sync_once`, then a local notification | Signed APK from a self-hosted F-Droid repo or Obtainium; Play testing track optional |
| Linux | Socket held while the app runs (tray, autostart) | Flatpak or AppImage |
| Windows | Same as Linux | Installer; unsigned builds trigger SmartScreen warnings |
| macOS, iOS (later) | APNs, plus PushKit and CallKit on iOS | App Store or TestFlight |

Push payloads carry no content and no sender, only "wake up". UnifiedPush keeps
phones without Google services fully supported.

### Command-line client and local API

`post` (crate `post-cli`) is a supported client, not only a test harness: it is
how bots and scripts use Post. A bot is an ordinary account with its own devices
and keys. Because messages are end-to-end encrypted, an automation API cannot live on
the server; it runs wherever the bot's core runs.

**One method set, two front ends.** The core's public API (above) is exposed as a set
of named methods with typed parameters and results, defined once in `post-api`.

- The CLI maps each subcommand to one method: `post send-text --to alice "hi"`
  calls `send_text`. Output is human-readable by default; `--json` prints the
  method's result as JSON, and failures exit non-zero with a JSON error.
- `post daemon` keeps the core running and serves the same methods as JSON-RPC
  2.0, with events (new messages, membership changes, security notices) pushed as
  JSON-RPC notifications after the client calls `subscribe`.

A wrapper in any language spawns `post daemon --stdio`, or connects to its socket,
and exchanges JSON lines. There is no native library to bind.

**Transports.**

| Transport | Use | Access control |
|---|---|---|
| stdin/stdout | A wrapper that spawns the daemon as a child process | Only the parent process can talk to it |
| Unix socket (Windows: named pipe) | A long-running daemon shared by several local tools | File mode `0600` in a `0700` directory, owned by the user |
| WebSocket on `127.0.0.1` | Languages or sandboxes without socket support; off by default | Requires a random bearer token from a `0600` file; binding a non-loopback address needs an explicit flag |

**Schema.** `post daemon --schema` prints an OpenRPC document generated from the
Rust types, so wrappers can generate bindings and stay in sync. The API is versioned
(`post.v1`); within a version, changes are additive only. Unknown fields in
requests are rejected, so typos fail loudly.

**Headless key storage.** Servers have no OS keyring. The database key comes from,
in order: `--db-key-file` (32 raw bytes, refused unless mode `0600`), systemd
credentials (`$CREDENTIALS_DIRECTORY`), or an interactive passphrase stretched with
Argon2id. There is no unencrypted mode.

**Bot accounts.**

- Registration uses an invite like any account. An admin can mark an invite as a bot
  invite; the account's devices then carry the `bot` flag in the chain, and the
  chain rules require every later device of that account to carry it too.
- Clients show a bot badge on such accounts everywhere they appear, including group
  member lists, so people know when an automated account can read a conversation.
- Bots follow the same group policy as people: an admin must add them to a group.

**Threat model note.** The local API has the full power of the account: whoever can
reach the socket or token can read and send everything that account can. It is
protected by operating-system permissions, so it is as secure as the user account
the daemon runs under. Run bots under their own system user.

## 12. Deployment

```
Caddy :443
  chat.example.org → postd :8080          WebSocket and HTTPS
  rtc.example.org  → livekit-server :7880    call signalling
livekit-server: UDP 7882 media, TCP 7881 fallback, TURN/TLS 5349
```

- Three containers in one pod: Caddy, `postd`, `livekit-server`. Calls arrive in a
  late phase, so the first deployments are Caddy and `postd` only.
  - `postd` ships as a minimal OCI image (static binary on a distroless base).
    Caddy and LiveKit use their official images.
  - The reference deployment is a Podman pod managed by systemd through Quadlet
    files, with a compose file as an alternative. Kubernetes works but is not needed
    at this size.
  - Exactly one replica. SQLite and the blob directory live on one persistent volume.
  - LiveKit needs UDP reachable from clients. Use its single-port UDP mode so only
    UDP 7882 and TURN/TLS 5349 need mapping. Hosts that block inbound UDP cannot run
    calls.
- State is `/var/lib/post/post.db` and `/var/lib/post/blobs/`, mounted from
  the volume.
- Backups: a nightly SQLite snapshot (or Litestream). Clients keep their own
  history, so losing the server database loses only undelivered messages and the
  account registry.
- `postd` runs as a non-root user in a read-only container filesystem. Config is
  one TOML file, overridable by environment variables.

### App updates

Apps find out about new releases from their own Post server and download them from
it. A community's devices update without an app store, and no third party learns who
runs Post or when. The server only distributes: it cannot make a build devices will
accept. Built in phase 5b for Linux (`crates/post-release`, `post-server/src/releases.rs`,
`post-core/src/update.rs`).

- **A release key held offline**: a hybrid Ed25519 + ML-DSA-65 key like a device
  chain key. Its public half is built into every app (`post-release/release-key.pub`);
  a build without one accepts no release. Its secret half is sealed with a passphrase
  (Argon2id, then XChaCha20-Poly1305) on the maintainer's machine.
- **Manifests.** Each release has, per platform, a JSON manifest
  `{ version, platform, files: [{ name, size, sha256 }], min_version, notes,
  released_at_ms }`. The release key signs its exact bytes under the label
  `post/release/v1`; both signatures must verify.
- **Building and signing** (`scripts/release.sh`). `release.sh new X.Y.Z` tags the
  pushed commit and pushes the tag. CI then builds each platform's files and an
  unsigned manifest, and leaves them in a draft GitHub release. The tag message is
  the notes; a `Min-Version:` line sets `min_version`. CI never holds the key.
  `release.sh sign` takes the newest draft. It checks every file against its
  manifest, signs each manifest with the key file (`~/post-release.key` by
  default), uploads the signatures and publishes the release.
- **Distribution.** Every hour `postd` asks GitHub for the latest published
  release. It verifies the manifest, downloads and checks each file, and only
  then serves the release from its releases directory. An operator can also copy
  a release in by hand. Apps fetch `GET /v1/releases/<platform>` (the manifest
  and signature as signed) and
  `GET /v1/releases/<platform>/<version>/<file>` over plain HTTPS. That
  connection is not tied to an account, and range requests let a download
  resume.
- **Checks on the device**, before anything is installed: both signatures verify
  under the built-in key; the version is newer than the running one (no downgrades,
  and a version once seen is never accepted with different contents); each file's
  size and SHA-256 match the manifest, checked again just before installing.
- **Installing on Linux.** Post installs per user from a tarball:
  `~/.local/share/post/versions/<version>/`, with `current` linking to the one the
  desktop entry starts. The first install runs the tarball's `install.sh`. An update
  unpacks beside the running version, accepting only plain files and folders with
  plain names. It then swaps `current` in one rename, and keeps the version before
  for going back. A copy run from anywhere else (a development build, a
  distribution package) only says that an update exists. The tarball is packed
  reproducibly: sorted names, fixed owners and timestamps, no gzip timestamp.
- **Other platforms**, later: Windows runs the signed installer; Android hands the
  APK to the system installer, which also requires the same app signing key.
- **The user decides when.** With automatic updates on (the default), the app asks
  every four hours and downloads in the background. The new version installs when
  the user clicks Restart, or as they quit. With them off, the server is asked only
  from Settings. `min_version` lets a release mark older versions as no longer able
  to talk to the server (after a protocol change), which the app shows across the
  top of the window.
- **Key rotation**: a new release key is announced in a release signed by the old
  one. Losing the release key means shipping a new key out of band, so it is kept
  offline with a backup.

## 13. Sizing

| Item | Estimate |
|---|---|
| Devices | 100 accounts × about 2.5 = 250, so at most 250 sockets |
| Text | 1,000 messages a day is 1,000 log rows a day, whatever the group size |
| Largest group | About 250 leaves. A key-refresh commit is tens of kilobytes; a Welcome with the full tree is under about 1 MB. Measure in the spike |
| Key refresh | About 36 commits a day in the largest group |
| Blobs | 100 a day × 10 MB is about 1 GB a day, 30 GB at 30-day retention |
| Calls | A relayed 1:1 video call is about 1.5 GB of egress an hour |
| Host | 2 vCPU, 4 GB RAM, 100 GB disk. Bandwidth for video is the real constraint |

Nothing here strains one machine, and the same design holds to roughly a thousand
accounts. The first things to give are call bandwidth and commit traffic in one very
large group, not the database.

## 14. Security rules

For every change:

- No custom primitives and no custom protocol where a standard one exists.
- The server is untrusted. Verify chains, credentials and commits on the client
  before acting on them.
- Fail closed. A bad signature, a chain fork or a policy-breaking commit is rejected
  and shown to the user, never skipped quietly.
- Never accept a chain that does not extend the one already held.
- Sign and verify exact bytes. Never re-serialise before verifying.
- Every derivation and signature uses a unique, versioned label (`post/.../v1`).
- Persist state before transmitting. Acknowledge only after committing.
- Verify, then discard. A client stores only decrypted content. It never persists a
  received ciphertext or an MLS signature, and no feature may export either. Outbox
  entries are deleted once the server accepts them.
- Each key has one purpose: chain keys do not sign MLS leaves, and exporter labels
  are never reused.
- Nothing secret or content-bearing in logs or push payloads.
- Keep the session and the anonymous connection apart. Anything about a
  conversation goes on the anonymous connection; the server refuses it on the
  session. Never add a server table that links a device or account to a group.

## 15. Testing

- **In-process integration:** the real server and several real cores in one
  `cargo test`, driven through the CLI client's code paths.
- **Fault injection:** crash at each step of receive, send and commit, then restart
  and check convergence. Race commits from several devices.
- **Property tests** on chain verification: forks, rollbacks, removed signers,
  stripped or swapped signatures.
- **Fuzzing** (`cargo-fuzz`) on every decoder that touches server-supplied bytes.
- **Known-answer tests** for hybrid signatures and the attachment format.
- **Adversarial server tests:** a test server that withholds chain links, injects
  unknown devices, replays and reorders. Each must produce a client-side rejection.
- **Deploy check:** assert the TLS handshake used the hybrid group.

## 16. Build order

| Phase | Work | Done when |
|---|---|---|
| 0 | Spikes (below) | Each has a yes/no answer |
| 1 | `post-proto`, `post-identity` | Chain and hybrid-signature tests pass. **Done 2026-11-10.** |
| 2 | Server core with metadata protection (anonymous connection, access keys, tokens, mailboxes, ephemeral relay), `post-api` and the CLI built on it: register, login, directory, KeyPackages, DMs with edits (all versions kept) and deletes, profiles (display names, per-conversation names, typing template, note); `--json` output and `post daemon --stdio` | Two CLI devices exchange, edit and delete DMs and see each other's profiles in an in-process test, and a test drives one through the JSON-RPC daemon. **Done 2026-11-10**; see "Phase 2 status" below |
| 3 | Multi-device: linking, self group, device add and remove, chain-head gossip, private nicknames (stored in the self group) | A linked device joins all conversations; a removed one is cut off. **Done 2026-11-10**; see "Phase 3 status" below |
| 4 | Groups: policy, metadata, leave, stale devices, key refresh; typing indicators and presence | Adversarial server tests pass; typing and presence reach online members and are never stored. **Done 2026-11-11**; see "Phase 4 status" below |
| 5 | Flutter app, text only: Linux, then Windows, then Android | Daily use between real devices |
| 5b | App updates over the wire (section 12, "App updates"): signed release manifests from the server, update detection, installing per platform | A device installs a signed update from the server and refuses an unsigned, modified or older one |
| 6 | Attachments, profile pictures, and history sharing for new members and new devices | Resumable upload and download on all three platforms; a member added to a `Shared` group sees its history |
| 7 | Android push and background sync | Messages arrive with the app closed |
| 8 | Calls: 1:1 audio, then video, then group | Rekey on membership change verified |
| 9 | Receipts, reactions, search, recovery key | |
| 10 | External review; ciphersuite codepoint migration | |

### Phase 2 status (2026-11-10)

Built: `post-proto`, `post-mls`, `post-server` (`postd`), `post-core`,
`post-api`, `post-cli` (`post`), container and Quadlet files in `deploy/`.
All tests pass in release and debug builds; clippy is clean.

Verified:

- End to end in one process: registration (invites, bot invites, unique
  usernames), DMs, replies, edits with every version kept, sender-only edit and
  delete, deletes removing every version, profiles, per-conversation names,
  nicknames, safety numbers, live delivery through wakes, restart from disk, wrong
  database key refused.
- A bot driving the client over JSON-RPC: methods, events, error codes, schema.
- Server protections against a raw protocol client: session and anonymous requests
  never mix; unknown groups and wrong access keys answer the same; tokens are
  required, unforgeable and single-use; no server table links an identity to a
  group.
- The real binaries against each other, and a server log free of addresses and
  content.
- The client negotiates TLS 1.3 with `X25519MLKEM768` (against a public endpoint).
- The container image (36 MB, distroless, non-root) runs read-only with all
  capabilities dropped and the key as a Podman secret; clients register and message
  through it, data survives a restart, and SIGTERM stops it cleanly.

Deferred, by design or to later phases:

- Chain-head gossip checks (phase 3); the `sender_chain` field is already sent.
- Group creation, commits other than DM creation, key refresh, typing and presence
  in clients (phase 4). The server already relays ephemeral signals.
- Daemon WebSocket transport, Windows named pipes, passphrase-derived database keys.
- Phone timings from spike 1; the deployment itself on a real host with Caddy.

Spike 1's libcrux/RustCrypto interop check now runs in the test suite
(`crates/post-mls/tests/interop.rs`).

### Phase 3 status (2026-11-10)

Built: linking by code with the server rendezvous, the self group, membership that
follows device chains, device listing, naming and removal, nickname and device-name
sync, chain-head gossip checks, `DevicesChanged` events, and the matching API methods
and CLI commands (`post link new`, `post link add`, `post devices`). Protocol
version 3. Profile databases now carry a schema version with ordered migrations.

Verified:

- End to end in one process: a linked device joins the account's DM and self group,
  gets the account's profile and nicknames, receives and sends in the DM, appears as
  the same account to the other side, and is included from the start in
  conversations created later. Nickname changes and clears on either device reach
  the other. No security notice is raised along the way, and the other side is told
  that a device was added.
- Removal: the server refuses the removed device at once; the other side accepts the
  removal commit after checking it against the chain, learns of the removal from the
  next message's chain head, and the conversation goes on.
- Unit tests: link codes and channels, self-group ordering rules, migration of a
  version 1 database, gossip comparison, rendezvous slots, and every protocol
  variant round-tripping.
- The real binaries: `link new` and `link add` between two CLI profiles, refusal to
  approve without a terminal or `--yes`, device names on both devices, removal, and a
  removed device refused at login.

Deferred:

- Signing a device out from itself (section 5, "Removing a device").
- History for a newly linked device (phase 6, with attachments).
- A QR code in the CLI; the app will show one.

### Phase 4 status (2026-11-11)

Built: named groups with admins (create, add and remove people, rename, admins,
history setting, members list), leaving by proposal, weekly key refresh and
`refresh_keys`, stale device removal and return, rejoining a group after removal,
typing indicators and presence, and the matching API methods and CLI commands
(`post group ...`, `post presence`, `post typing`, `post refresh-keys`).
Protocol version 4. Profile databases reach schema version 5; the server database
gains schema versioning (version 2).

Verified:

- End to end in one process: group creation with a multi-device member, profiles
  reaching people who never shared a conversation, admin-only changes, handing the
  group on, removal and being added back (also while offline), leaving with all of
  an account's devices, the only admin's hand-over, simultaneous key refreshes from
  two members (counted on the server: exactly one commit per leaf, none on the next
  sync), stale devices removed by their account and by an admin but never an
  account's last device, and their return; typing rendered with the sender's template
  and the viewer's nickname, presence changes, invisible sending nothing after
  "offline", and none of it entering a group log.
- Adversarial members, with real MLS commits and proposals (`adversarial_tests.rs`):
  a non-admin adding, renaming, promoting themselves or removing someone; an admin
  changing the conversation kind or orphaning the group; unlisted devices; a third
  account in a DM; another account in a self group; a proposal to remove someone
  else; a Welcome listing an unlisted device; a forged Welcome reusing a group's ID.
  The legitimate counterparts pass.
- The real binaries: group commands end to end, including refusal of a non-admin and
  of leaving without a terminal or `--yes`.

Deferred:

- Away after 10 minutes idle, and "offline" on a clean shutdown: app behaviour
  (phase 5).
- The setting to see standard typing text instead of others' templates (phase 5).
- Adversarial *server* tests beyond what the protocol checks already cover
  (withheld chain links, forked directories and unauthorized KeyPackages are
  rejected by code tested in phases 1 to 3); a test server that reorders or replays
  log entries on purpose is still to build.

### Spikes to run first

These are the assumptions the design rests on that have not been tested:

1. **OpenMLS hybrid suite at size.** Build a 250-leaf group; measure commit and
   Welcome sizes and timings on a mid-range Android phone. Confirm the storage trait
   can share a transaction with application tables. Check whether the published
   audit covered the post-quantum suites.
   *Done on desktop, go (`spikes/mls-pq/RESULTS.md`): all operations work; warm-tree
   refresh commits are about 20 KB at 250 leaves; a newly built group's first
   commits are up to 300 KB until members refresh; storage shares SQLCipher
   transactions and survives a rollback. Still open: phone timings, audit scope.*
2. **Flutter and the core.** `flutter_rust_bridge` on Linux, Windows and Android,
   including calling the core from a background isolate started by a push.
3. **LiveKit E2EE.** Per-participant raw keys and key-index rotation in the Flutter
   SDK on all three platforms, against a self-hosted server. Confirm the frame
   cipher's key length.
4. **ML-DSA library.** Choose between `aws-lc-rs` (already present for TLS) and
   RustCrypto `ml-dsa`. Both are young; the hybrid rule limits the damage of a flaw.
   *Decided: RustCrypto `ml-dsa` 0.1.2, the version OpenMLS itself depends on, so
   clients link one ML-DSA implementation. Pure Rust, so it cross-compiles to Android
   without a C toolchain.*
5. **Android incoming calls.** Full-screen ring from a high-priority push with the
   app killed, across at least two vendors.

## 17. Open decisions

- Whether any member may create invites, or only admins.
- Whether display names and avatars stay visible to the server (simple, assumed
  here) or move to encrypted profiles shared in conversations.

Decided:

- No disappearing messages for now. Message history is kept on devices.
- New members can receive group history, per group setting, a week at a time on
  request; newly linked devices can go back through all of their history (section 6,
  "History for new members").
- DMs and group chats now; Discord-style community servers later, on the shape in
  section 6, "Future: community servers".

## 18. References

- RFC 9420 (MLS) and RFC 9750 (MLS architecture)
- draft-ietf-mls-pq-ciphersuites
- OpenMLS: https://github.com/openmls/openmls
- LiveKit encryption: https://docs.livekit.io/transport/encryption/
- FIPS 203 (ML-KEM), FIPS 204 (ML-DSA)
