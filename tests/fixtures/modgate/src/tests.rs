//! Fixture: the body of the `#[cfg(test)] mod tests;` declared in
//! `src/lib.rs`. The gate is in that file, not this one, so the crate named
//! below is attributed to test code only if module resolution carries the
//! gate down.

use file_test_crate::assert_ok;

#[test]
fn builds_a_thing() {
    assert_ok(super::build());
}
