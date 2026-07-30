//! Fixture build script: the only target compiled against the
//! `[build-dependencies]` table, and so the only one that can justify an entry
//! in it.

fn main() {
    build_crate::probe();
    // A `[dev-dependencies]` entry named only from here. The build script
    // cannot link it either, but build-script evidence says nothing about
    // which of the other two tables an entry belongs in, so the check stays
    // quiet rather than guessing between them.
    build_only_dev_crate::probe();
}

// A build script has no test harness: `cargo test` compiles no `#[test]`
// function here, so this one is compiled by nothing at all. What it names is
// still build-script code and nothing else — calling it a dev-dependency would
// move the entry out of the only table anything reads it from.
#[test]
fn probes() {
    build_test_crate::assert_ok();
}
