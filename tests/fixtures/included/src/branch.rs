//! The layout a naive fix would resolve `pub mod branch;` to: beside the file
//! the `include!` was *written in*, rather than beside the file it named.
//! Nothing compiles this, so it is a dead file and is reported as one.

pub fn beside_the_includer() {}
