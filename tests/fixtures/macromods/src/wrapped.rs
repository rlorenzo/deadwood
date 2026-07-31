//! Reached only through `wrapper! { pub mod wrapped; }`.
//!
//! The `pub fn` nothing references pins the other half of the treatment: a
//! macro-reached file is spared from the dead-file check but its items are
//! not admitted to resolution, so nothing in here can become an
//! `unused_pub_item` finding — the module path the macro gives it is
//! unknowable without expansion.
pub fn nobody_references_this() {}
