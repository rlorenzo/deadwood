//! A private module: nothing outside the crate can reach what is in here, so
//! findings about it are certain rather than advisory. That makes it the
//! control for `public-api`, which must not reach past what it names.

mod hidden;

/// Out of external reach behind a private module, so an unused re-export here
/// is dead with certainty.
pub use hidden::Buried;

/// `pub`, but in a module no outside code can name: an `items` listing for
/// `surface::api::*` must leave this reported.
pub fn internal_leftover() {}
