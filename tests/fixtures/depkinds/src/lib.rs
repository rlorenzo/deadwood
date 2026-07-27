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

/// The same unit tests written out of line: the gate is here, the code is in
/// `src/outline_tests.rs`, and nothing in that file says what it is.
#[cfg(test)]
mod outline_tests;

// One file under three declarations. The gated ones sit either side of the
// ungated one so that neither pop order can decide the answer: whichever
// declaration is read first, the ungated one is what this file is.
#[cfg(test)]
#[path = "shared_view.rs"]
mod view_before_the_ungated_one;

#[path = "shared_view.rs"]
mod shared_view;

#[cfg(test)]
#[path = "shared_view.rs"]
mod view_after_the_ungated_one;

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
