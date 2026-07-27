//! A library, where the public surface is a root: consumers Deadwood cannot
//! see call it, so a path written inside a surface item is not evidence that
//! anything is dead.
//!
//! Being a root does not exempt an item from being reported. `exported::entry`
//! below is on the surface and nothing in the workspace names it, which is the
//! advisory finding Deadwood has always made for libraries. What changes is
//! that `internal::worker` — which only `entry` calls — is *not* dragged down
//! with it.

pub mod exported;

mod hidden;
mod internal;
