//! The only mention of the entry below in the whole package, which is what
//! makes its `[dependencies]` entry a misplaced one rather than an unused one.
//! (The crate name is deliberately not spelled in this comment: a doc mention
//! is attributed to no target, and would silence the finding.)

#[test]
fn uses_the_crate_the_manifest_misplaces() {
    test_only_crate::check();
}
