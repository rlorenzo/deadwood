//! A private module of a library: nothing outside the crate can name
//! `probe::hidden::*` however `pub` it is, so these are judged like a
//! binary's items.

/// Reported under `warn.toml`, and silenced by `declared.toml`, which lists it
/// under `[public-api]` — the project saying a consumer we cannot see reaches
/// it after all.
pub fn declared() -> u32 {
    6
}

/// Reported: `pub` in a private module, and only the crate's own tests reach
/// it. This is the shape the kind exists for.
pub fn undeclared() -> u32 {
    7
}

/// Not reported either, and for a reason worth separating from the one above:
/// nothing outside the crate can name `support`, but the surface item that
/// calls it is a root in *both* walks, so a build with no tests in it reaches
/// this too. The surface is not merely excluded from this kind — what it
/// reaches is excluded with it.
pub fn support() -> u32 {
    5
}
