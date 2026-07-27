//! A private module, so nothing here is a root by construction: outside code
//! cannot name `declared::hidden::plugin` however `pub` it is.
//!
//! `[public-api]` naming it outright is the only thing that can make it one,
//! which is what that setting is for — a surface Deadwood cannot see, because
//! a macro re-exports it, a build script generates the `pub use`, or a
//! `#[path]` pulls the file into a crate that does. Undeclared, `plugin` is
//! reported and `support` falls with it.

pub fn plugin() -> u32 {
    support()
}

pub fn support() -> u32 {
    4
}
