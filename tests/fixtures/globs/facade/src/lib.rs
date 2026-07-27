//! The reproducer from
//! [#25](https://github.com/rlorenzo/deadwood/issues/25), and the shapes
//! around it that must keep their answers.

mod imported;
mod inner;
mod other;

/// A re-export: every `pub` item in `inner`, and every `pub mod` under it, is
/// nameable as `facade::*` by a consumer Deadwood cannot see.
pub use inner::*;
