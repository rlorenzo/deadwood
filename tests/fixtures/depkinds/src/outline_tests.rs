//! The unit tests of `src/lib.rs`, written out of line.
//!
//! Nothing in this file says it is test code — the `#[cfg(test)]` that makes
//! it so is written on the `mod` declaration back in `src/lib.rs`. What it
//! names is therefore a dev-dependency, exactly as if it had been written
//! inline.

use outline_test_crate::assert_ok;

#[test]
fn builds_a_thing() {
    assert_ok(crate::build());
}
