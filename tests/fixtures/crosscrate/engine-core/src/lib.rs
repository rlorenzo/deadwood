pub mod api;

/// Used from `app` through `engine_core::start()`.
pub fn start() -> u32 {
    1
}

/// Dead: no path in either member reaches it.
pub fn never_started() -> u32 {
    2
}
