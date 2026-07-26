//! Fixture: a file two `mod` declarations in `src/lib.rs` reach, one behind
//! `#[cfg(test)]` and one not. The ungated declaration is what counts: this
//! file is compiled into runtime builds, so the crate it names is correctly
//! declared under `[dependencies]`.

fn helper() -> u32 {
    both_ways_crate::value()
}
