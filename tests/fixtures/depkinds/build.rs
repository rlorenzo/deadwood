//! Fixture build script: the only target compiled against the
//! `[build-dependencies]` table, and so the only one that can justify an entry
//! in it.

fn main() {
    build_crate::probe();
}
