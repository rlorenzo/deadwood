//! One of two binaries whose crate roots are both spelled `crate`.
//!
//! `shared` here and `shared` in `two.rs` are two different items with the same
//! kind, the same name and the same module path, in two different files of one
//! package. Nothing but the file tells them apart, which is why the file is
//! still the first thing the baseline matches on.

pub fn shared() {}

fn main() {}
