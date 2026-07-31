//! The crate root, in the package directory rather than in `src/`.
pub mod live;

/// Reached from `tests/it.rs`, and reaching into the nested package, so the
/// only findings this fixture produces are the dead files it is about.
pub fn go() {
    inner::entry();
}
