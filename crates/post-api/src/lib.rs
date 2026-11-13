//! Post's local API (design section 11, "Command-line client and local
//! API"; docs/phase-1-api.md).
//!
//! The methods are post-core's own `Core` methods marked `#[api]`, and the
//! types are core's types: nothing is copied here. `post-apigen` generates
//! the dispatch from those annotations (`api.generated.rs`, included at the
//! end), with a params struct per method. The CLI, the JSON-RPC daemon and
//! the app all call [`call`] or [`dispatch`], so they cannot drift apart.
//!
//! Parameters refuse unknown keys, so a typo fails instead of being
//! ignored. `openrpc.generated.json` describes every method and type.

use post_core::{self as core, Core};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The API version.
pub const VERSION: &str = "post.v1";

/// What a method needs before it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Works offline, from local data.
    Local,
    /// Needs the account and a connection.
    Connection,
    /// Creates the account; needs no prior connection.
    Registration,
}

/// A method's description, for the CLI and the daemon.
#[derive(Clone, Copy, Debug)]
pub struct MethodInfo {
    /// Its name: the `Core` method's.
    pub name: &'static str,
    /// The first paragraph of its doc comment.
    pub summary: &'static str,
    /// What it needs.
    pub needs: Needs,
}

/// Find a method by name.
pub fn method(name: &str) -> Option<&'static MethodInfo> {
    METHODS.iter().find(|m| m.name == name)
}

/// The OpenRPC document describing every method and type.
pub fn openrpc() -> Value {
    serde_json::from_str(include_str!("openrpc.generated.json")).expect("the generated OpenRPC file is JSON")
}

/// Why a call failed, before it is given a JSON-RPC code and a sentence.
#[derive(Debug)]
pub enum CallError {
    /// No such method.
    UnknownMethod(String),
    /// The parameters did not fit.
    InvalidParams(String),
    /// The core refused or failed.
    Core(core::Error),
    /// The result could not be written as JSON (a bug, not the caller's).
    Internal(String),
}

impl From<core::Error> for CallError {
    fn from(e: core::Error) -> Self {
        Self::Core(e)
    }
}

/// An API error, with a JSON-RPC error code.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    /// JSON-RPC error code. -32601: unknown method; -32602: bad parameters;
    /// -32603: internal; 1000 and up: Post errors (see [`code_of`]).
    pub code: i64,
    /// Human-readable message.
    pub message: String,
}

/// JSON-RPC code for each core error.
pub fn code_of(e: &core::Error) -> i64 {
    use core::Error as E;
    match e {
        E::NotRegistered => 1001,
        E::AlreadyRegistered => 1002,
        E::NotConnected | E::Network(_) => 1003,
        E::Server(_) => 1004,
        E::InvalidInput(_) => 1005,
        E::NotFound(_) => 1006,
        E::NotAllowed(_) => 1007,
        E::UnknownUser(_) => 1008,
        E::IdentityChanged(_) => 1009,
        E::PeerUnavailable(_) => 1010,
        E::NoTokens => 1011,
        E::WrongKey => 1012,
        _ => 1099,
    }
}

impl From<core::Error> for ApiError {
    fn from(e: core::Error) -> Self {
        Self { code: code_of(&e), message: e.to_string() }
    }
}

impl From<CallError> for ApiError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::UnknownMethod(m) => Self { code: -32601, message: format!("unknown method {m}") },
            CallError::InvalidParams(m) => Self { code: -32602, message: format!("invalid parameters: {m}") },
            CallError::Internal(m) => Self { code: -32603, message: m },
            CallError::Core(e) => e.into(),
        }
    }
}

/// A method's parameters; a missing (null) object counts as empty.
fn params<T: DeserializeOwned>(v: Value) -> Result<T, CallError> {
    let v = if v.is_null() { Value::Object(Default::default()) } else { v };
    serde_json::from_value(v).map_err(|e| CallError::InvalidParams(e.to_string()))
}

/// Parameters for a method that takes none: null or `{}`.
fn no_params(v: Value) -> Result<(), CallError> {
    match v {
        Value::Null => Ok(()),
        Value::Object(o) if o.is_empty() => Ok(()),
        Value::Object(o) => Err(CallError::InvalidParams(format!(
            "this method takes no parameters, not {}",
            o.keys().map(String::as_str).collect::<Vec<_>>().join(", ")
        ))),
        _ => Err(CallError::InvalidParams("parameters are an object".into())),
    }
}

/// A method's result as JSON.
fn result(v: impl Serialize) -> Result<Value, CallError> {
    serde_json::to_value(v).map_err(|e| CallError::Internal(e.to_string()))
}

/// Run one method, connecting first when it needs a connection and there
/// is none. Its parameters are checked before that, so a mistake never
/// waits on the network. A local method that turns out to need the server
/// (looking up a user this device has never seen) connects and tries once
/// more.
pub async fn call(core: &Core, name: &str, params: Value) -> Result<Value, ApiError> {
    let info = method(name).ok_or_else(|| CallError::UnknownMethod(name.to_owned()))?;
    check(name, &params)?;
    if info.needs == Needs::Connection && !core.is_connected().await {
        core.connect().await?;
    }
    match dispatch(core, name, params.clone()).await {
        Err(CallError::Core(core::Error::NotConnected | core::Error::Network(_))) if info.needs == Needs::Local => {
            core.connect().await?;
            Ok(dispatch(core, name, params).await?)
        }
        r => Ok(r?),
    }
}

include!("api.generated.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_method_is_described() {
        let doc = openrpc();
        let described: Vec<&str> = doc["methods"].as_array().expect("methods").iter().filter_map(|m| m["name"].as_str()).collect();
        let listed: Vec<&str> = METHODS.iter().map(|m| m.name).collect();
        assert_eq!(described, listed);
        assert_eq!(doc["info"]["version"], VERSION);
        assert!(METHODS.iter().all(|m| method(m.name).is_some() && !m.summary.is_empty()));
    }

    #[test]
    fn parameters_are_checked() {
        assert!(no_params(Value::Null).is_ok());
        assert!(no_params(serde_json::json!({})).is_ok());
        assert!(matches!(no_params(serde_json::json!({ "typo": 1 })), Err(CallError::InvalidParams(_))));
        let p: Result<RenameDeviceParams, _> = params(serde_json::json!({ "device": "ab", "name": "phone", "colour": 1 }));
        assert!(matches!(p, Err(CallError::InvalidParams(m)) if m.contains("unknown field")));
        let p: CreateInviteParams = params(Value::Null).expect("defaults");
        assert_eq!((p.bot, p.days), (false, 7));
    }

    #[test]
    fn errors_get_json_rpc_codes() {
        assert_eq!(ApiError::from(CallError::UnknownMethod("x".into())).code, -32601);
        assert_eq!(ApiError::from(CallError::InvalidParams("x".into())).code, -32602);
        assert_eq!(ApiError::from(CallError::Core(core::Error::UnknownUser("x".into()))).code, 1008);
    }
}
