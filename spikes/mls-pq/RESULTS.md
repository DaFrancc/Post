# Spike 1: OpenMLS with the hybrid post-quantum suite

Run 2026-11-10 on an AMD Ryzen 7 5800X (desktop). Android numbers are still to do;
expect a mid-range phone to be roughly 5 to 10 times slower.

Versions: `openmls` 0.9.2, `openmls_libcrux_crypto` 0.4.0, `openmls_rust_crypto` 0.6.0,
`openmls_sqlite_storage` 0.3.0, SQLCipher 4.6.1.

Suite: `MLS_128_MLKEM768X25519_AES256GCM_SHA384_Ed25519`. OpenMLS uses the provisional
codepoint `0x004E`; the IETF draft assigns `0x0011`. In OpenMLS this suite's KEM is
X-Wing (draft 06).

## Verdict

Go. Every operation works on both providers. Timings are small. Sizes are fine once a
group's tree has filled in (see "Cold and warm trees").

## Results

`cargo run --release` (group built in one commit by one device, so the tree is cold):

| provider | leaves | KeyPackage | create: commit / time | Welcome / join time | refresh: create / process | refresh commit | add-1: commit / Welcome | remove-1: commit / time | 100 B msg |
|---|---|---|---|---|---|---|---|---|---|
| libcrux | 3 | 2.6 KB | 9.3 KB / 2.7 ms | 9.3 KB / 1.1 ms | 1.2 / 0.9 ms | 6.3 KB | 9.1 KB / 9.4 KB | 6.3 KB / 1.5 ms | 318 B |
| libcrux | 10 | 2.6 KB | 30.1 KB / 4.8 ms | 30.1 KB / 2.0 ms | 2.2 / 1.4 ms | 16.8 KB | 19.6 KB / 21.4 KB | 16.8 KB / 2.0 ms | 318 B |
| libcrux | 50 | 2.6 KB | 136.8 KB / 25.0 ms | 137.0 KB / 7.1 ms | 5.2 / 3.3 ms | 65.8 KB | 68.3 KB / 78.0 KB | 65.8 KB / 5.8 ms | 318 B |
| libcrux | 100 | 2.6 KB | 268.6 KB / 65.7 ms | 268.8 KB / 13.2 ms | 8.4 / 5.8 ms | 124.8 KB | 127.6 KB / 146.8 KB | 124.8 KB / 8.1 ms | 318 B |
| libcrux | 250 | 2.6 KB | 661.6 KB / 284.1 ms | 661.9 KB / 31.1 ms | 17.7 / 11.5 ms | 300.1 KB | 302.8 KB / 351.1 KB | 300.1 KB / 17.7 ms | 318 B |
| rustcrypto | 250 | 2.6 KB | 661.6 KB / 233.6 ms | 661.9 KB / 21.2 ms | 16.2 / 8.8 ms | 300.1 KB | 302.8 KB / 351.1 KB | 300.1 KB / 17.0 ms | 318 B |

The two providers produce identical sizes and similar timings at every size; only
250 is shown for RustCrypto.

`cargo run --release -- --warm 16 64 128` (every member joins and commits one refresh
first, as weekly refreshes will do in real use):

| leaves | refresh, cold tree | refresh, warm tree | create / process | add-1 commit | Welcome / join time |
|---|---|---|---|---|---|
| 16 | 23.8 KB | 11.1 KB | 2.1 / 1.6 ms | 14.8 KB | 44.8 KB / 2.7 ms |
| 64 | 82.1 KB | 15.8 KB | 5.0 / 3.7 ms | 19.6 KB | 170.8 KB / 9.4 ms |
| 128 | 157.3 KB | 18.1 KB | 9.6 / 6.4 ms | 22.1 KB | 338.7 KB / 18.8 ms |

## Cold and warm trees

A group built in one commit has empty internal tree nodes. The next committer must
then encrypt its new path secret to every other leaf separately, at about 1.2 KB of
X-Wing ciphertext each: 300 KB at 250 leaves.

Each commit fills the nodes on its sender's path. Once members have refreshed, a
commit needs about log2(n) encryptions: 18 KB at 128 leaves, growing about 2.4 KB per
doubling, so roughly 20 KB at 250. This matches the design's estimate.

So the large commits are a short phase after a big group is created, and they
shrink as weekly refreshes come in. Not a problem at this scale, so no change is
planned. If it ever matters, the creator can spread the initial adds over a few
commits, or members can refresh soon after joining.

A Welcome carries the whole tree, so it grows linearly: about 340 KB at 128 leaves
on a warm tree, so roughly 700 KB at 250.

## Storage (`cargo run --release --bin storage`)

- `SqliteStorageProvider` takes any borrowed `rusqlite::Connection` and never opens
  its own transactions, so it runs inside a transaction the app controls.
- Test: a database-backed device processes a commit and writes an app row in one
  transaction, then rolls back. MLS state and the app row both revert. Processing
  the same commit again then succeeds. This is the crash-safety rule in design
  section 11.
- Works on SQLCipher (`rusqlite` feature `bundled-sqlcipher`). The file on disk is
  encrypted.
- Build note: `bundled-sqlcipher-vendored-openssl` needs a full Perl (`FindBin`),
  which this machine lacks (`sudo dnf install perl-FindBin`). The spike linked the
  system OpenSSL instead. Android builds will need the vendored option, or a
  prebuilt OpenSSL.

## Provider choice (`cargo run --release --bin interop`)

- The providers use separate KEM code: libcrux uses `libcrux-ml-kem` (formally
  verified); RustCrypto uses the `ml-kem` and `x-wing` crates.
- `openmls_libcrux_crypto` accepts this suite but does not list it in
  `supported_ciphersuites()`, so upstream does not test it on that provider.
- Interop test: a libcrux device and a RustCrypto device in one group, each side
  creating it in turn, with key refreshes and messages both ways and matching
  exporter secrets. Passes.
- Decision: use the libcrux provider, as in the design, and carry the interop test
  into `postern-core`'s test suite so an upgrade that breaks agreement fails CI.

## Still open from spike 1

- Timings on a real mid-range Android phone.
- Whether the SRLabs audit of OpenMLS covered the post-quantum suites.
