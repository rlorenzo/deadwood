//! Fixture: the channels through which code can name a dependency.
//!
//! Nothing here compiles — none of these crates exist — but all of it parses,
//! which is all Deadwood needs. Each item below is the *only* mention of one
//! dependency, so losing any single channel turns that dependency into a
//! false positive.
#![doc = include_str!("../README.md")]

extern crate extern_crate_only;

use used_crate::Thing;

pub use reexport_crate::Exported;

/// A doc example is compiled as a doctest, so what it uses is used.
///
/// ```
/// use doc_example_crate::helper;
/// helper();
/// ```
pub fn documented() {}

/// The attribute path names one crate; another crate is named only inside a
/// string argument, the shape derive macros use for real paths.
#[attr_path_crate(rename_all = "camelCase")]
pub struct Wired {
    #[attr_path_crate(with = "attr_string_crate::codec")]
    pub field: u8,
}

pub fn thing() -> Thing {
    Thing
}

/// Spelled by the alias from `Cargo.toml`, not by the package name.
pub fn renamed() {
    motor::spin();
}

/// Inside macro input, which is never expanded and so never resolved.
pub fn in_macro() {
    println!("{}", macro_body_crate::VALUE);
}
