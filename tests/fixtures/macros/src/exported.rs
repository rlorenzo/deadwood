/// Alive only because macro token streams count as uses of every item of
/// that name: nothing else in the crate can resolve to it.
pub fn only_in_macro() -> u32 {
    1
}

/// Alive only because `opaque_scope` has a glob import that cannot be
/// followed, so the bare `shadowed()` written there might be this.
pub fn shadowed() -> u32 {
    2
}
