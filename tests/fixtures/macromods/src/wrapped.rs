//! Reached only through `wrapper! { pub mod wrapped; }`.
//!
//! The `pub fn` nothing references pins the other half of the treatment: a
//! macro-reached file is spared from the dead-file check but its items are
//! not admitted to resolution, so nothing in here can become an
//! `unused_pub_item` finding — the module path the macro gives it is
//! unknowable without expansion.
pub fn nobody_references_this() {}

/// The paths a spliced file writes still count. This one is the only
/// reference `only_the_macro_mod_calls_me` has anywhere in the workspace.
pub fn calls_out() {
    crate::reached_from_macro_mod::only_the_macro_mod_calls_me();
}
