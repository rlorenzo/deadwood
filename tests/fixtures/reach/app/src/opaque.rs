//! An opaque mention is a *root*, not merely a use.
//!
//! `mentioned` is named nowhere Deadwood can resolve: its one appearance is
//! inside macro input, which is not expanded, so it counts as a use of every
//! item of that name. That mention sits inside `dead_caller`, which nothing
//! reaches — and reading it as an ordinary edge would let the dead caller take
//! `mentioned` down with it. The mention would be evidence we had already
//! admitted we could not read, turned into a finding.

pub fn mentioned() -> u32 {
    3
}

/// Reported: nothing names this one, which is the ordinary finding.
pub fn dead_caller() {
    println!("{}", mentioned as usize);
}
