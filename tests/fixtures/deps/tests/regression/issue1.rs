//! Reached only by the `automod::dir!` expansion in `tests/regression.rs`:
//! no `mod` declaration anywhere names this file, and it is still compiled.

use regression_only_crate::helper;

#[test]
fn uses_the_hidden_dev_dependency() {
    helper();
}
