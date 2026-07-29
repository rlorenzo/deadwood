// Depth 9: rustc compiles this and Deadwood does not read it, so it is
// reported dead. That is the cap paying its own price in the direction the
// cap is for — a file is spared only by an `include!` that was actually read.
pub fn past_the_cap() {}
