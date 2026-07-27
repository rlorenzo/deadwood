//! The target half of the split. A test, bench or example target is test code
//! in its entirety, not only the functions carrying `#[test]` — this one has
//! no `#[test]` function at all, and its `fn main` must still be a test entry
//! point. Read `fn main` as an ordinary entry point and `from_target` below
//! becomes reachable in both walks and the finding disappears.

mod support;

fn main() {
    if support::from_target() != 4 {
        panic!("from_target is broken");
    }
}
