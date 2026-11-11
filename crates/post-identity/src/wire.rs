//! Protobuf encoding of chain links. Hand-written prost types, so building
//! needs no `protoc`.
//!
//! Signatures cover the exact bytes received. Decoders additionally require
//! the input to be canonical (it must re-encode to the same bytes), so each
//! signed object has exactly one encoding and carries no unknown fields.

#![allow(missing_docs)]

use prost::Message;

use crate::Error;

#[derive(Clone, PartialEq, Message)]
pub struct WireDeviceEntry {
    #[prost(bytes = "vec", tag = "1")]
    pub chain_pub_ed25519: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub chain_pub_mldsa65: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub mls_sig_pub: Vec<u8>,
    #[prost(bool, tag = "4")]
    pub bot: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct WireGenesis {
    #[prost(string, tag = "1")]
    pub server_domain: String,
    #[prost(string, tag = "2")]
    pub username: String,
    #[prost(message, optional, tag = "3")]
    pub device: Option<WireDeviceEntry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WireAddDevice {
    #[prost(message, optional, tag = "1")]
    pub device: Option<WireDeviceEntry>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WireRemoveDevice {
    #[prost(bytes = "vec", tag = "1")]
    pub device_id: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WireRotateMlsKey {
    #[prost(bytes = "vec", tag = "1")]
    pub device_id: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub mls_sig_pub: Vec<u8>,
}

pub mod wire_link {
    #[derive(Clone, PartialEq, prost::Oneof)]
    pub enum Op {
        #[prost(message, tag = "10")]
        Genesis(super::WireGenesis),
        #[prost(message, tag = "11")]
        AddDevice(super::WireAddDevice),
        #[prost(message, tag = "12")]
        RemoveDevice(super::WireRemoveDevice),
        #[prost(message, tag = "13")]
        RotateMlsKey(super::WireRotateMlsKey),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct WireLink {
    #[prost(uint32, tag = "1")]
    pub version: u32,
    /// Empty in the genesis link, 32 bytes otherwise.
    #[prost(bytes = "vec", tag = "2")]
    pub account_id: Vec<u8>,
    #[prost(uint64, tag = "3")]
    pub seq: u64,
    /// Empty in the genesis link, 32 bytes otherwise.
    #[prost(bytes = "vec", tag = "4")]
    pub prev_hash: Vec<u8>,
    #[prost(uint64, tag = "5")]
    pub timestamp_ms: u64,
    #[prost(bytes = "vec", tag = "6")]
    pub signer: Vec<u8>,
    #[prost(oneof = "wire_link::Op", tags = "10, 11, 12, 13")]
    pub op: Option<wire_link::Op>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WireSignedLink {
    /// The encoded [`WireLink`]. Signatures and hashes cover these bytes.
    #[prost(bytes = "vec", tag = "1")]
    pub link: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub sig_ed25519: Vec<u8>,
    #[prost(bytes = "vec", tag = "3")]
    pub sig_mldsa65: Vec<u8>,
    /// Countersignature by the device an `AddDevice` link adds. Empty otherwise.
    #[prost(bytes = "vec", tag = "4")]
    pub cosig_ed25519: Vec<u8>,
    #[prost(bytes = "vec", tag = "5")]
    pub cosig_mldsa65: Vec<u8>,
}

/// Decode `bytes` as `M`, rejecting anything that is not the canonical encoding.
pub fn decode_canonical<M: Message + Default>(bytes: &[u8], what: &'static str) -> Result<M, Error> {
    let m = M::decode(bytes).map_err(|_| Error::Malformed(what))?;
    if m.encode_to_vec() != bytes {
        return Err(Error::NonCanonical);
    }
    Ok(m)
}
