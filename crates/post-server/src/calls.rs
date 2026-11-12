//! Tokens for joining calls on the LiveKit server beside `postd` (design
//! section 10, "Calls").
//!
//! LiveKit carries every call's media, encrypted end to end with keys only
//! the members derive, so LiveKit relays frames it cannot read. `postd`
//! decides who may join: anyone presenting the group's current access key
//! gets a short-lived token for that call's room, under the random
//! participant ID they chose for the call. The server learns that a call is
//! happening in a group, as it learns that messages are, and never who is
//! in it.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// The LiveKit server, and the host's limits for calls.
#[derive(Clone)]
pub struct CallConfig {
    /// LiveKit's WebSocket URL, as apps reach it (`wss://…`).
    pub url: String,
    /// LiveKit's API key name.
    pub api_key: String,
    /// LiveKit's API secret, which signs tokens.
    pub api_secret: Zeroizing<Vec<u8>>,
    /// The most each participant may send, kbit/s.
    pub max_bitrate_kbps: u32,
    /// The most participants a call may have.
    pub max_participants: u32,
}

/// How long a token lets someone start joining. LiveKit renews tokens for
/// participants already in a room.
const TOKEN_LIFETIME_SECS: u64 = 600;

/// The LiveKit room for a call in a group: named from both, so a call ID
/// from one group never reaches another group's room.
pub fn room_name(group_id: &[u8], call_id: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"post/call-room/v1");
    h.update((group_id.len() as u32).to_be_bytes());
    h.update(group_id);
    h.update(call_id);
    hex::encode(&h.finalize()[..16])
}

/// A LiveKit access token (an HS256 JWT) for `participant` to join `room`,
/// with the host's limits.
pub fn token(config: &CallConfig, room: &str, participant: &str, now_secs: u64) -> String {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header = b64.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = serde_json::json!({
        "iss": config.api_key,
        "sub": participant,
        "nbf": now_secs.saturating_sub(10),
        "exp": now_secs + TOKEN_LIFETIME_SECS,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": true,
            "canSubscribe": true,
            // LiveKit's data messages are outside the media encryption, so
            // calls send nothing that way: signalling goes through MLS.
            "canPublishData": false,
        },
        // Applied when the room is first created.
        "roomConfig": {
            "maxParticipants": config.max_participants,
            "emptyTimeout": 60,
            "departureTimeout": 20,
        },
    });
    let body = b64.encode(serde_json::to_vec(&claims).unwrap_or_default());
    let signing_input = format!("{header}.{body}");
    let mut mac = Hmac::<Sha256>::new_from_slice(&config.api_secret).expect("HMAC takes any key length");
    mac.update(signing_input.as_bytes());
    format!("{signing_input}.{}", b64.encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CallConfig {
        CallConfig {
            url: "wss://rtc.example.org".into(),
            api_key: "postkey".into(),
            api_secret: Zeroizing::new(b"a secret long enough for hmac".to_vec()),
            max_bitrate_kbps: 1500,
            max_participants: 8,
        }
    }

    /// Prints a token for checking against a real LiveKit:
    /// `POST_TEST_LIVEKIT_SECRET=… cargo test -p post-server print_token -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_token() {
        let secret = std::env::var("POST_TEST_LIVEKIT_SECRET").expect("POST_TEST_LIVEKIT_SECRET");
        let c = CallConfig { api_secret: Zeroizing::new(secret.into_bytes()), api_key: "post".into(), ..config() };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_secs();
        println!("TOKEN={}", token(&c, &room_name(b"group", &[1; 16]), "0123456789abcdef0123456789abcdef", now));
    }

    #[test]
    fn rooms_are_bound_to_their_group() {
        let call = [7u8; 16];
        assert_eq!(room_name(b"group-a", &call), room_name(b"group-a", &call));
        assert_ne!(room_name(b"group-a", &call), room_name(b"group-b", &call));
        assert_ne!(room_name(b"group-a", &call), room_name(b"group-a", &[8u8; 16]));
        assert_eq!(room_name(b"g", &call).len(), 32);
    }

    #[test]
    fn tokens_are_signed_jwts_with_the_limits() {
        let c = config();
        let t = token(&c, "room1", "participant1", 1_000_000);
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(parts.len(), 3);
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let claims: serde_json::Value = serde_json::from_slice(&b64.decode(parts[1]).expect("b64")).expect("json");
        assert_eq!(claims["iss"], "postkey");
        assert_eq!(claims["sub"], "participant1");
        assert_eq!(claims["video"]["room"], "room1");
        assert_eq!(claims["video"]["canPublishData"], false);
        assert_eq!(claims["roomConfig"]["maxParticipants"], 8);
        assert_eq!(claims["exp"].as_u64(), Some(1_000_000 + TOKEN_LIFETIME_SECS));
        // The signature is HMAC-SHA256 over the first two parts.
        let mut mac = Hmac::<Sha256>::new_from_slice(&c.api_secret).expect("key");
        mac.update(format!("{}.{}", parts[0], parts[1]).as_bytes());
        mac.verify_slice(&b64.decode(parts[2]).expect("b64")).expect("verifies");
    }
}
