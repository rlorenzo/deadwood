//! This module is `pub` all the way up from the crate root of a library, so
//! a consumer outside the workspace can write `paths::facade::Published`.
//! Nothing here goes through the re-export, which is exactly what a public
//! API surface looks like — and so is not reported.

mod internal;

pub use internal::Published;
