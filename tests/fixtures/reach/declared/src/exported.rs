//! `pub` under a `pub mod` from the crate root: surface by construction for
//! any library, and named by `[public-api]` in `public-api.toml` when the
//! project wants to say so outright.

/// Reported in the unconfigured run — nothing in the workspace names it —
/// and silenced by `[public-api]`. Either way it is a root, so what it calls
/// stays quiet.
pub fn entry() -> u32 {
    crate::internal::worker()
}
