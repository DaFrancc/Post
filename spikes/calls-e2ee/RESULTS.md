# Spike 2: end-to-end encrypted call audio with LiveKit on Linux

Run 2026-11-11 on Fedora (PipeWire 1.6.11), AMD Ryzen 7 5800X, against a local
`livekit-server` in dev mode.

Versions: `livekit-server` 1.13.11, `livekit_client` (Flutter) 2.13.2, `flutter_webrtc`
1.6.3, Flutter 3.47.9.

## Verdict

Go. With a key per sender (`BaseKeyProvider.create(sharedKey: false)` and raw 32-byte
keys from `setRawKey`), a participant holding the sender's key decodes their audio,
and one holding a wrong key decodes nothing. Design section 10 holds, with the changes
under "Findings".

## Results

`integration_test/e2ee_test.dart`: Alice publishes her microphone encrypted with her
key; Bob holds Alice's key; Eve holds a wrong one. After 8 seconds:

| | packets | samples decoded | concealed | audio energy |
|---|---|---|---|---|
| Alice (sent) | 394 | 8.1 s captured | | 0.58 |
| Bob, right key | 392 | 387,840 | 0% | 0.49 |
| Eve, wrong key | 393 | 0 | 100% | 0 |

The server marks the track `kGcm`: it relays the encrypted frames without being able
to read them.

## Findings

- **Cipher: AES-GCM with a 128-bit key.** Read from LiveKit's libwebrtc
  (`api/crypto/frame_crypto_transformer.cc`): the key set by the app is stretched
  (PBKDF2-SHA256, 100,000 iterations, or HKDF) to 128 bits, and ratcheting also
  derives 128-bit keys; 256-bit keys are supported by the cipher code but not
  produced. The IV is 12 bytes built from the SSRC, the RTP timestamp and a counter;
  a two-byte trailer carries the IV length and key index. Audio frames leave one
  byte (the Opus TOC) in the clear. AES-128 holds today; it is below the 256-bit
  margin the rest of Post keeps against quantum attacks, so it is a known limit.
- **Linux: the microphone must be named.** Publishing with no device chosen, WebRTC
  opens the default source and drops it at once (PipeWire shows the stream come and
  go), and nothing is sent. Passing a `deviceId` from `Hardware.enumerateDevices()`
  works. Linux device IDs are the device labels.
- **The "decrypting fine" event can be lost.** `livekit_client` attaches its
  frame-cryptor state listener only after the cryptor is running, and the native side
  reports only changes, so the first `ok` (or `decryptionFailed`) can fire before
  anyone listens. Judge by the media (decoded versus concealed samples) instead.
- **Receiver statistics are empty on Linux** (`RTCRtpReceiver.getStats()`); the peer
  connection's own statistics carry the `inbound-rtp` report.
- **Build:** newer clang turns a warning in flutter_webrtc's own code
  (`-Wunused-but-set-global`) into an error under the Linux template's `-Werror`.
  `-Wno-error=unused-but-set-global` in `linux/CMakeLists.txt` keeps it a warning.
- **Speaking activity reaches the server.** WebRTC sends the audio level in a header
  extension outside the encrypted frame, so the server can see when each participant
  speaks (not what they say). Worth turning off where the SDK allows.

## Running it

```sh
podman run --rm -d --name livekit-spike --network host \
  docker.io/livekit/livekit-server:v1.13.11 --dev --bind 127.0.0.1 --node-ip 127.0.0.1
cd spikes/calls-e2ee && flutter test integration_test/e2ee_test.dart -d linux
podman stop livekit-spike
```
