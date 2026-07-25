pub mod api;

/// Used from `app` through `engine_core::start()`.
pub fn start() -> u32 {
    1
}

/// Dead: no path in any member reaches it.
pub fn never_started() -> u32 {
    2
}

/// Reached only from `aliased`, which spells this crate `motor`.
pub fn aliased_only() -> u32 {
    3
}
