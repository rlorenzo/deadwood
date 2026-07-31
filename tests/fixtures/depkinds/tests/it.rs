//! Fixture integration test. A test target links the dev-dependencies, so an
//! entry only this file names is declared one table too high.

#[test]
fn builds_a_thing() {
    let thing = shared_crate::make();
    test_only_crate::assert_ok(thing);
    depkinds::build();
    // Also named by the library, which is what decides its table.
    library_and_test_dev_crate::assert_ok();
    // A separate crate: `src/lib.rs`'s rename does not reach here, so this is
    // the dev-dependency of that name and nothing else.
    aliased_crate::assert_ok();
    use_aliased_crate::assert_ok();
}
