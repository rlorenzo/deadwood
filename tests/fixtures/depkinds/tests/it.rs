//! Fixture integration test. A test target links the dev-dependencies, so an
//! entry only this file names is declared one table too high.

#[test]
fn builds_a_thing() {
    let thing = shared_crate::make();
    test_only_crate::assert_ok(thing);
    depkinds::build();
}
