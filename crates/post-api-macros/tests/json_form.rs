//! The JSON form `#[api]` gives types (docs/phase-1-api.md, "The JSON form"),
//! and the annotations it accepts and removes, under `deny(missing_docs)`.

#![deny(missing_docs)]

use post_api_macros::api;
use serde_json::{Value, json};

/// Something with every kind of field.
#[api]
#[derive(Debug, PartialEq)]
pub struct Thing {
    /// An ID.
    pub id: String,
    /// When (Unix ms).
    #[api(unix_ms)]
    pub at: u64,
    /// A name, if it has one.
    pub name: Option<String>,
    /// How many; may be left out.
    #[api(default)]
    pub count: u32,
}

/// What others see of someone.
#[api]
#[derive(Debug, PartialEq)]
pub enum Presence {
    /// Not there.
    Offline,
    /// There, but not to be disturbed.
    DoNotDisturb,
}

/// Something that happened.
#[api]
#[derive(Debug, PartialEq)]
pub enum Event {
    /// A message was deleted.
    MessageDeleted {
        /// Its conversation.
        conversation: String,
        /// The message.
        message_id: String,
    },
    /// Some events were missed.
    Lagged,
}

/// A stand-in for `Core`.
pub struct Core;

impl Core {
    /// Make an invite.
    #[api(needs = connection)]
    pub fn create_invite(
        &self,
        /// For a bot account.
        #[api(default)]
        bot: bool,
        /// Days until it expires.
        #[api(default = 7)]
        days: u32,
    ) -> (bool, u32) {
        (bot, days)
    }
}

fn read<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, String> {
    serde_json::from_value(v).map_err(|e| e.to_string())
}

#[test]
fn a_struct_is_an_object_with_its_field_names() {
    let t = Thing { id: "ab".into(), at: 5, name: None, count: 0 };
    assert_eq!(serde_json::to_value(&t).expect("write"), json!({"id": "ab", "at": 5, "name": null, "count": 0}));
}

#[test]
fn optional_and_default_fields_may_be_left_out() {
    assert_eq!(read::<Thing>(json!({"id": "ab", "at": 5})), Ok(Thing { id: "ab".into(), at: 5, name: None, count: 0 }));
}

#[test]
fn a_required_field_may_not_be_left_out() {
    assert!(read::<Thing>(json!({"id": "ab"})).is_err());
}

#[test]
fn unknown_keys_are_refused() {
    let e = read::<Thing>(json!({"id": "ab", "at": 5, "colour": 1})).expect_err("unknown key");
    assert!(e.contains("unknown field"), "{e}");
    let e = read::<Event>(json!({"type": "message_deleted", "conversation": "c", "message_id": "m", "x": 1})).expect_err("unknown key");
    assert!(e.contains("unknown field"), "{e}");
}

#[test]
fn a_unit_enum_is_its_variant_in_snake_case() {
    assert_eq!(serde_json::to_value(Presence::DoNotDisturb).expect("write"), json!("do_not_disturb"));
    assert_eq!(read::<Presence>(json!("offline")), Ok(Presence::Offline));
}

#[test]
fn an_enum_with_fields_is_tagged_with_type() {
    let e = Event::MessageDeleted { conversation: "c".into(), message_id: "m".into() };
    assert_eq!(serde_json::to_value(&e).expect("write"), json!({"type": "message_deleted", "conversation": "c", "message_id": "m"}));
    assert_eq!(serde_json::to_value(&Event::Lagged).expect("write"), json!({"type": "lagged"}));
    assert_eq!(read::<Event>(json!({"type": "lagged"})), Ok(Event::Lagged));
}

#[test]
fn a_method_keeps_working_without_its_annotations() {
    assert_eq!(Core.create_invite(true, 3), (true, 3));
}
