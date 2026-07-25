//! Fixture build script: the only place a `[build-dependencies]` entry can
//! legitimately be used, so it has to be scanned like any other target.

fn main() {
    build_only_crate::probe();
}
