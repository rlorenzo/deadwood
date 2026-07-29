include!("i2.rs");
// Depth 1 of a chain of nine. `src/deps.rs` reads eight `include!`s deep and
// so does the module tree, on purpose: two readers with two caps would read
// one crate to two depths.
