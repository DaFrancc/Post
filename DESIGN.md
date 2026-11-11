# PQ Chat: Design Spec

A small, private, end-to-end encrypted messaging service with post-quantum (hybrid) key exchange. Written in Go.

## Scope

- Up to ~100 users, invite-only
- 1:1 DMs and group chats
- File sharing (~100 uploads/day)
- Voice/video calls (a few concurrent at most)
- One account per user, usable on multiple devices (phone + desktop)
- ~1,000 texts/day

### Non-goals (for now)

- Hiding metadata (the server knows who talks to whom and when)
- Federation with other servers
- Syncing old message history to newly linked devices
- Scaling beyond a single server

## Core principles

1. The server never sees plaintext or private keys. It is a dumb relay and storage box.
2. All key exchange is **hybrid**: classical (X25519) + post-quantum (ML-KEM-768). Both secrets feed one KDF, so breaking either alone gets an attacker nothing.
3. No custom crypto primitives. Compose standard ones, following published specs (Signal's PQXDH and Double Ratchet) for what gets signed and what goes into each KDF.
4. The design must stay secure if the attacker has the full source code. Only keys are secret.

## Crypto stack

| Purpose | Algorithm | Go package |
|---|---|---|
| Classical key agreement | X25519 | `crypto/ecdh` |
| PQ key encapsulation | ML-KEM-768 | `crypto/mlkem` (Go 1.24+) |
| Classical signatures | Ed25519 | `crypto/ed25519` |
| PQ signatures | ML-DSA-65 | `github.com/cloudflare/circl/sign/mldsa/mldsa65` |
| KDF | HKDF-SHA256 | `crypto/hkdf` |
| AEAD | ChaCha20-Poly1305 | `golang.org/x/crypto/chacha20poly1305` |
| Randomness | OS CSPRNG | `crypto/rand` |

Hybrid signatures: an identity signature is valid only if **both** the Ed25519 and ML-DSA-65 signatures verify.

Size notes: ML-KEM-768 encapsulation key = 1184 bytes, ciphertext = 1088 bytes. ML-DSA-65 signature ≈ 3.3 KB. Keep prekey bundles and device lists compact accordingly.

## Identity and devices

### Keys

- **Identity key (per account):** hybrid signing key pair (Ed25519 + ML-DSA-65). Signs the device list. Its public half is what users verify via safety numbers.
- **Device keys (per device):**
  - Device signing key (Ed25519): authenticates the device to the server and signs its own prekeys
  - Signed prekey: X25519, rotated periodically (e.g. weekly)
  - PQ signed prekey: ML-KEM-768, rotated periodically
  - One-time prekeys: batches of X25519 and ML-KEM-768 keys, each used once

Decision: the identity private key is copied to every linked device, so any device can link or remove others. Trade-off: compromising one device compromises the identity. Acceptable for this scale; revisit later if needed.

### Device list

```go
type Device struct {
    ID         string
    SigningKey []byte // Ed25519 public
    X25519     []byte // signed prekey (public)
    MLKEM      []byte // PQ signed prekey (public)
}

type DeviceList struct {
    UserID  string
    Version uint64 // monotonically increasing
    Devices []Device
    Sig     HybridSig // identity key over all fields above
}
```

- Clients cache each contact's latest device list and **reject any list with a lower Version** (blocks rollback attacks by the server).
- A change in the identity key itself must show a loud warning, like Signal's "safety number changed."

### Linking a new device

1. New device generates its device keys and shows a QR code (device public keys + a one-time pairing secret).
2. Existing device scans it, opens an encrypted channel keyed by the pairing secret through the server.
3. Existing device sends the identity private key, adds the new device to the list, bumps Version, signs, and uploads.

### Removing a device

Any remaining device publishes a new signed list without it. Contacts drop sessions with the removed device.

### Verification

Safety numbers derived from both users' identity public keys, shown as digits and a QR code. Scanning in person marks the contact verified.

## Sessions (1:1, per device pair)

Sessions are between **device pairs**, not user pairs. Alice-phone ↔ Bob-desktop is its own session.

### Handshake

Follow the PQXDH spec. Alice fetches Bob's device prekey bundle (identity key, signed prekey, PQ prekey, one one-time prekey if available), verifies signatures, then:

```go
eph, _ := ecdh.X25519().GenerateKey(rand.Reader)
dh1 := /* X25519 secrets per PQXDH: identity/signed/one-time combos */
ek, _ := mlkem.NewEncapsulationKey768(bobPQPrekey)
ssPQ, ct := ek.Encapsulate()

root, _ := hkdf.Key(sha256.New, concat(dh1..., ssPQ), nil, "pqchat-v1-root", 32)
```

Alice sends `eph.PublicKey()`, `ct`, and the IDs of the prekeys used in the first message header. Bob reconstructs `root`.

### Ratchet

- **Phase 1:** symmetric ratchet. Each direction has a chain key. Per message: `msgKey = HKDF(chain, "msg")`, `chain = HKDF(chain, "chain")`, then delete the old chain key. Gives forward secrecy.
- **Phase 2:** full Double Ratchet with periodic ML-KEM re-encapsulation mixed into the root key. Gives post-compromise security, including against quantum attackers.

Keys are derived per direction by including sender and receiver device IDs in the HKDF label.

### Message envelope

```go
type Envelope struct {
    Version     uint8
    From        string // sender device ID
    To          string // recipient device ID
    Counter     uint64 // message number in this chain
    Handshake   []byte // only on the first message(s) of a session
    Ciphertext  []byte
}
```

- Nonce = counter. All header fields are passed as AEAD associated data.
- Reject any counter already seen (allow a small skip window for out-of-order delivery, storing skipped message keys with a cap).
- **Persist the counter/chain state before sending.** A crash must never cause nonce reuse.

### Sending to a user

Encrypt once per device in the recipient's verified device list, **plus your own other devices** so they stay in sync.

## Groups

- Group membership and metadata (name, members) are sent as encrypted control messages, never stored in plaintext on the server.
- **Phase 1: pairwise fan-out.** Encrypt each message to every member device over existing 1:1 sessions. 100 users × 2 devices = ~200 encryptions per message, fine at this scale.
- **Phase 2 (if needed): sender keys.** Each member generates a group chain key and distributes it pairwise once. Messages are then encrypted once and uploaded once. Rotate sender keys when anyone leaves.

## Files

1. Generate a random 32-byte file key.
2. Encrypt the file in 64 KB chunks with ChaCha20-Poly1305:
   - Nonce = chunk index
   - Associated data = 1 byte: `1` for the final chunk, `0` otherwise (prevents truncation)
   - Each chunk prefixed with its uint32 ciphertext length
3. Upload the encrypted blob once; server returns a blob ID.
4. Send `{blobID, fileKey, fileName, size, mimeType, sha256}` inside a normal encrypted message (fanned out like any message).
5. Receiver downloads, decrypts chunk by chunk, rejects if the final chunk isn't flagged or any chunk fails.

Voice notes and video messages are just files.

## Calls

- **Signaling** (offer/answer/ICE candidates) goes over the existing encrypted message channel.
- **Call key:** the caller picks a random 16-byte call ID, sends it in the call offer. Each side derives per-direction keys:

```go
func callKey(sessionKey, callID []byte, from, to string) []byte {
    k, _ := hkdf.Key(sha256.New, sessionKey, callID, "call-v1:"+from+">"+to, 32)
    return k
}
```

- **Media transport:** `pion/webrtc` (codecs, jitter buffer, congestion control). Don't build this.
- **Frame encryption (SFrame-style):** encrypt each media frame with the call key before it enters WebRTC's transport. Nonce = frame sequence number, header as associated data. This keeps calls post-quantum even though WebRTC's own DTLS-SRTP is classical.
- **Replay protection:** sliding window of the last 64 sequence numbers; drop duplicates and anything older. Late packets in-window still decrypt.
- **Rekey:** hash the call key forward every ~10 seconds, signaled by an epoch number in the frame header. Delete old keys.
- **NAT/IP privacy:** relay calls through the server's TURN (`pion/turn`) so peers don't see each other's IP.
- **Group calls:** full mesh for up to ~4 participants. Larger would need an SFU that forwards encrypted frames without decrypting.

## Server

Single Go binary.

### Components

- **WebSocket API** (`github.com/coder/websocket`) for messaging and real-time delivery
- **SQLite** (`modernc.org/sqlite`, pure Go, easy cross-compiling)
- **Blob storage** on local disk for encrypted files
- **TURN** server (`pion/turn`)
- **Push** sender for APNs and FCM

### Auth

- Registration requires an invite code.
- Login: server sends a random challenge, device signs it with its device signing key. No passwords.
- Each WebSocket connection is bound to one device ID.

### Data model (rough)

```
users(id, identity_pub, created_at)
device_lists(user_id, version, blob)            -- signed list, stored as-is
devices(id, user_id, signing_pub, push_token)
prekeys(device_id, kind, key_id, pub, used)     -- signed, PQ, one-time
queue(id, to_device, envelope, created_at)      -- deleted on ack
blobs(id, path, size, created_at, expires_at)
invites(code, created_by, used_by, expires_at)
```

### Retention

- Queued messages: deleted once acknowledged by the recipient device; dropped after 30 days if never fetched.
- Blobs: deleted after 30 days.

### Push notifications

Phones kill background sockets, so the server sends a push to wake the app. **Push payloads contain no content**, just "you have new messages." The app wakes, connects, and fetches from the queue. Incoming calls on iOS need PushKit + CallKit.

## Clients

- **`core` package (shared):** all crypto, sessions, device lists, envelope encoding, local encrypted storage. No UI code.
- **Desktop:** Wails or Fyne, importing `core` directly.
- **Mobile:** `gomobile bind` the `core` package into a library; thin native UI (Swift/Kotlin) or Flutter on top.
- **Local storage:** SQLite, encrypted at rest with a key from the OS keychain/keystore.

## Suggested repo layout

```
/cmd/server        server main
/cmd/cli           minimal CLI client for testing
/core/crypto       hybrid KEM, hybrid sigs, KDF helpers, AEAD wrappers
/core/identity     identity keys, device lists, safety numbers
/core/session      PQXDH handshake, ratchet, envelope
/core/files        chunked file encryption
/core/calls        call key derivation, frame encryption, replay window
/core/store        local encrypted storage
/server/api        websocket handlers
/server/db         sqlite schema + queries
/server/blobs      blob storage
/server/push       APNs/FCM
/server/turn       TURN setup
```

## Sizing

- Texts: 1,000/day is negligible.
- Files: ~100/day × ~10 MB avg ≈ 1 GB/day ≈ 30 GB at 30-day retention.
- Calls: a TURN-relayed 1:1 video call ≈ 1.5 GB egress per hour.
- Target host: 2 vCPU, 4 GB RAM, 100 GB disk VPS (~$10–20/month). Watch the bandwidth cap; relayed video is what consumes it.

## Build order

1. `core/crypto` with tests (hybrid KEM, hybrid signatures, AEAD wrappers, test vectors)
2. Server: invites, device auth, prekey upload/fetch, message queue over WebSocket
3. CLI client: register, 1:1 text with PQXDH + symmetric ratchet
4. Device lists, device linking via QR, self-sync to own devices
5. Safety numbers and verification
6. Groups (pairwise fan-out)
7. Files (chunked encryption, blob upload/download)
8. Desktop UI
9. Mobile app + push notifications
10. 1:1 calls (pion/webrtc + frame encryption + TURN)
11. Full Double Ratchet with ML-KEM re-encapsulation
12. Group calls (mesh), sender keys if group performance needs it

## Security rules (for every change)

- Never reuse a (key, nonce) pair. Persist counters before using them.
- Every byte in a header is authenticated as associated data.
- Every key derivation uses a unique, versioned label (`"pqchat-v1-..."`) and includes direction where relevant.
- Delete old chain/message keys immediately after use.
- Treat everything from the server as untrusted: verify signatures on device lists and prekeys before use.
- Prefer failing closed: if a signature or MAC fails, drop the message and surface an error.
