//! Reached by a plain `use crate::imported::*;` in `other`, which re-exports
//! nothing: an import brings names in for the importing module's own use, so
//! it puts nothing on any surface and the cascade runs through it as before.

mod buried;

/// Reported: the glob that names it is an import, not a re-export.
pub fn from_import() -> u32 {
    5
}

/// Reported as an unused re-export, with `Stale` beside it: no glob exports
/// `imported`, so outside code cannot reach this and it has no excuse.
pub use buried::Stale;
