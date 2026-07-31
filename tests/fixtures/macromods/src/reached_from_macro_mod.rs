//! An ordinary module, in the `mod` tree, holding an item whose *only*
//! reference is written in `wrapped.rs` — a file no `mod` declaration names
//! and only a macro token stream claims.
//!
//! This is the direction the splice boundary must not cost anything. Sparing
//! `wrapped.rs` from the dead-file check while throwing away the paths it
//! writes does not lose a finding, it invents one: the item below is called,
//! and reporting it unused is a false positive.
pub fn only_the_macro_mod_calls_me() {}
