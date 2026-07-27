//! Glob-re-exported from a crate root nothing outside can name.

/// Not reported: `fn main` goes through the glob to reach it.
pub fn used_by_main() -> u32 {
    6
}

/// Reported: in a library the glob above would root this, and here there is no
/// surface for it to be rooted onto.
pub fn from_glob() -> u32 {
    7
}
