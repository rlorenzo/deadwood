//! A library crate whose `pub` surface is meant for consumers Deadwood cannot
//! see. Every unused-pub finding here is the advisory kind the `public-api`
//! setting exists to silence.

pub mod api;

mod generated;
mod internal;

/// On the crate root's public surface and referenced by nothing inside the
/// workspace. Covered by a `crates` listing but not by an `items` listing that
/// only names `surface::api::*`.
pub fn another_entry() {}
