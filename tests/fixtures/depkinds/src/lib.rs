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
        // A bare `#[test]` inside a module `#[cfg(test)]` already confined.
        // The module moved this code once; the attribute must not be a second,
        // separate answer about the same mention.
        nested_test_fn_crate::assert_ok();
    }
}

// A bare `#[test] fn` at module scope, with no `#[cfg(test)]` anywhere near it.
// This is how `clap_builder` writes `check_auto_traits`, and rustc leaves the
// function out of every build that is not a test build — verified: the same
// file compiles as a library and fails under `--test` when the crate it names
// does not exist.
#[test]
fn checks_a_thing() {
    // A `[dev-dependencies]` entry, correctly declared. Reporting it would be
    // a finding invented against a manifest that compiles.
    test_fn_dev_crate::assert_ok();
    // A `[dependencies]` entry no other code names: it belongs one table down,
    // and this is the finding the gap used to cost.
    test_fn_crate::assert_ok();
}

#[bench]
fn benches_a_thing() {
    bench_fn_crate::assert_ok();
}

// The boundary, in both shapes. Neither attribute confines a function, so both
// of these mentions are library code and both entries are where they belong.
#[should_panic]
fn panics_on_a_thing() {
    should_panic_crate::assert_ok();
}

#[harness::test]
fn drives_a_thing() {
    proc_macro_test_crate::assert_ok();
}
