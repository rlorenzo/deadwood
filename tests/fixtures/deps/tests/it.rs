//! Fixture integration test: a `[dev-dependencies]` entry used only here is
//! used, so test targets have to be scanned like any other.

#[test]
fn uses_the_dev_dependency() {
    test_only_crate::assert_ok();
}
