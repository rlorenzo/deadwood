//! A module under the glob that is not `pub`. `pub use inner::*;` re-exports
//! only what
//! is `pub` in `inner`, so nothing outside can name `deep` and the descent
//! stops here.

/// Reported: its only referrer is dead, and no consumer can reach it either.
pub fn buried() -> u32 {
    4
}

/// Not reported — the `pub use` in `inner.rs` carries it onto the surface
/// under a name of its own, and reaching that re-export reaches this.
pub struct Carried;
