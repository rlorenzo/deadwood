//! The attribute half of the split: a `#[test]` function is a test entry
//! point wherever it is written, including in an inline `#[cfg(test)] mod`
//! inside ordinary library code.

/// Reported. `main` does not reach it, the `#[test]` below does, and dropping
/// the test entry points from the root set is exactly what makes it
/// unreachable. Note what the finding does *not* say: this function is
/// referenced and alive, so "delete it" would be wrong — `pub(crate)` is the
/// answer, or a move behind `#[cfg(test)]`.
pub fn only_tests() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    /// Written without `assert_eq!` on purpose. A name in macro input is a
    /// *root*, so an assertion naming `only_tests` would keep it out of the
    /// kind entirely — which is the point `opaque.rs` makes.
    #[test]
    fn covers_it() {
        if super::only_tests() != 1 {
            panic!("only_tests is broken");
        }
    }
}
