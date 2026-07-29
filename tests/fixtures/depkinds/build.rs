//! Fixture build script: the only target compiled against the
//! `[build-dependencies]` table, and so the only one that can justify an entry
//! in it.

fn main() {
    build_crate::probe();
}

// A build script has no test harness: `cargo test` compiles no `#[test]`
// function here, so this one is compiled by nothing at all. What it names is
// still build-script code and nothing else — calling it a dev-dependency would
// move the entry out of the only table anything reads it from.
#[test]
fn probes() {
    build_test_crate::assert_ok();
}
