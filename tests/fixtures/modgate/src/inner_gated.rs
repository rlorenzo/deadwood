//! Fixture: a file declared with no gate at all that gates *itself* with an
//! inner attribute. `#![cfg(test)]` confines the whole file, and so every
//! module declared in it, to a test build — which is why the declaration
//! below is test-gated even though nothing is written on it.
//!
//! The file's own flag stays "not test-only": a detector reading this file can
//! see the inner attribute for itself, and `src/deps.rs` does. What it cannot
//! see is the gate on a declaration in *another* file, which is the whole
//! reason module resolution carries one down.
#![cfg(test)]

mod inherited;

fn helper() -> u32 {
    inherited::value()
}
