//! Spliced into `shared_view.rs`, which three declarations reach.
//!
//! The `include!` is read on the first walk of that file — under whichever
//! gated declaration got there first, so recorded confined — and the walk that
//! lifts the file has to lift this one too. `deeper.rs` beside it is the same
//! case for a `mod` declaration, and the two must not answer differently.

fn spliced_view() {}
