//! A second dead file, unrelated to `attic/dropped.rs` in every way a reader
//! would recognise and in none a matcher can see.
//!
//! In `unrelated.toml` this is the finding that must stay reported: the
//! baseline there records a dead file that is gone, this one is news, and the
//! two are one leftover on each side. Reading that as a move — the only reading
//! available to a rule with no content signal — would silence it.

pub struct Spare {
    pub label: &'static str,
}
