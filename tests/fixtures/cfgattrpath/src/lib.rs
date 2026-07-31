//! `built_from_git` is a `cfg` no build sets, so the second arm is the one
//! rustc takes here. Both arms name a real file, which is the case that has to
//! resolve to exactly one of them.
#[cfg_attr(built_from_git, path = "from_git/internals.rs")]
#[cfg_attr(not(built_from_git), path = "vendored/internals.rs")]
mod internals;

pub use internals::shared;
