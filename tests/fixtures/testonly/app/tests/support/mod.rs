//! In `tests/support/mod.rs` rather than `tests/support.rs`: cargo turns every
//! `tests/*.rs` file into a test target of its own, and this is a helper
//! module, not a target.

/// Reported. Nothing outside a test binary can name it, so `pub` here buys
/// nothing that `pub(crate)` would not.
pub fn from_target() -> u32 {
    4
}
