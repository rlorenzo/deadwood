//! Fixture: a file-backed child of the *inline* `#[cfg(test)] mod` in
//! `src/lib.rs`. An inline module is walked in place, so the gate has to be
//! inherited through the recursion for this file to come out as test code.
//! It names no dependency: what it pins is the flag module resolution computes.

fn deep_helper() -> u32 {
    super::super::build()
}
