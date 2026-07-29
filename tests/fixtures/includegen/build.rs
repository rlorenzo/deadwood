//! Writes the file `src/lib.rs` includes, so the crate compiles and the
//! `include!` in it is the real construct rather than a broken one.

use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out = env::var("OUT_DIR").expect("cargo sets OUT_DIR for a build script");
    fs::write(
        Path::new(&out).join("generated.rs"),
        "pub fn generated() {}\n",
    )
    .expect("the build script can write to OUT_DIR");
    println!("cargo::rerun-if-changed=build.rs");
}
