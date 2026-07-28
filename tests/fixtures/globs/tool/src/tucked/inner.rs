//! Under a module a named `pub use` re-exports, in a crate that has no public
//! surface for that to mean anything.

/// Reported: in a library the `pub use tucked::inner;` in `main.rs` would put
/// this module on the public surface and its only referrer being dead would
/// stop being evidence. A binary seeds the closure with no module at all, so
/// the finding stands exactly as it did.
pub fn from_named_reexport() -> u32 {
    8
}
