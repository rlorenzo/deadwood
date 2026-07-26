//! Fixture: declared by `mod inherited;` in `src/inner_gated.rs`, a file whose
//! own `#![cfg(test)]` confines it. Nothing is written on the declaration, and
//! this file is still test code.

pub fn value() -> u32 {
    1
}
