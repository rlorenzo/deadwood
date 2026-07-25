//! An inner `#![cfg(...)]` gates the whole file, not one item in it.
#![cfg(windows)]

pub fn from_inner_gate() {}
