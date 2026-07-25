//! Fixture: a `[target.'cfg(any())'.dependencies]` entry, which is compiled
//! by no target on any matrix and so is skipped rather than reported.
pub fn nothing() {}
