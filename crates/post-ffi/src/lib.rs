//! The bridge between the Flutter app and `post-core` (design section 11,
//! "Core"): an app-facing API that `flutter_rust_bridge` turns into Dart.
//!
//! Everything in [`api`] becomes a Dart function or class. Keep it thin: the
//! core owns protocol logic and state; this crate adapts types, opens the
//! profile with its key from the platform keystore, and streams events.
//!
//! `frb_generated` is written by `flutter_rust_bridge_codegen generate`
//! (run in `app/`); never edit it by hand.

pub mod api;
mod keystore;
mod spell_catalog;

#[allow(unsafe_code, missing_docs, clippy::unwrap_used)]
mod frb_generated;
