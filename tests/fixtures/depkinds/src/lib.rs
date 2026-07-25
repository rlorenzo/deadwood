//! Fixture: which manifest table each entry belongs in.
//!
//! Nothing here compiles — none of the crates named below exist — but all of
//! it parses, which is all Deadwood needs. Every entry is named exactly once,
//! from exactly one kind of code, so each one pins a different answer.

use shared_crate::Thing;

/// Builds a thing.
///
/// The example is compiled as a doctest, which links the dev-dependencies, so
/// what it names is correctly declared as one:
///
/// ```
/// use doc_only_crate::harness;
/// harness().check();
/// ```
pub fn build() -> Thing {
    stale_build_crate::helper();
    Thing
}

/// Unit tests live inside the library target and still link the
/// dev-dependencies, which is what makes this the check's hardest case.
#[cfg(test)]
mod tests {
    use cfg_test_crate::assert_ok;

    #[test]
    fn builds_a_thing() {
        assert_ok(super::build());
    }
}
