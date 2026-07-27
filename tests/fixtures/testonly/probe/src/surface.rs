//! `pub` under a `pub mod` from the crate root: surface by construction.

/// Not reported. Only the crate's own tests call it *here*, but a consumer
/// Deadwood cannot see may call it anywhere, and the public surface is a root
/// in both walks precisely so that this claim is never made about one.
pub fn exported() -> u32 {
    crate::hidden::support()
}
