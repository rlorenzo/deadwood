//! Private, and the two hops live under it. `first` is on the surface because
//! `lib.rs` re-exports it; `second` is on the surface because `first` does.

pub mod first;
pub mod second;
