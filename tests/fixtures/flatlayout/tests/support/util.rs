//! A stray helper under `tests/`. Cargo auto-discovers `tests/*.rs` but not
//! this, so no target root reaches it — and it must still not be a finding:
//! `<package>/tests/` is where deliberate test scaffolding lives.
pub fn helper() {}
