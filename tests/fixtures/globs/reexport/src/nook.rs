//! Private, and holding the two modules `lib.rs` re-exports by name. Nothing
//! outside can name `nook` itself, which is what makes the modules under it
//! unreachable until the third edge is followed.

pub mod plain;
pub mod renamed;
