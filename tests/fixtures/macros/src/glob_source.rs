/// Used through the crate root's `use glob_source::*`.
pub fn from_glob() -> u32 {
    1
}

/// Dead: the glob makes it visible in the crate root, but no path names it.
/// Expanding the glob rather than giving up is what keeps this reportable.
pub fn never_named() -> u32 {
    2
}
