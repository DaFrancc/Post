//! A second fixture file, read as the public module `kinds`, so API types
//! outside the crate root are named by their path.

use post_api_macros::api;

/// Which picture.
#[api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureKind {
    /// The round one beside a name.
    Avatar,
    /// The wide one atop a profile card.
    Banner,
}
