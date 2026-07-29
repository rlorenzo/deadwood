//! A dead file, and the one the baselines beside this fixture record.
//!
//! It has nothing in common with `spare.rs` — not a name, not a module, not a
//! line of its contents — and a baseline entry for it records none of that
//! either. What the entry records is `src/attic/dropped.rs`, and a path is all
//! there is.

pub fn dropped_helper(input: u32) -> u32 {
    input.saturating_sub(1)
}
