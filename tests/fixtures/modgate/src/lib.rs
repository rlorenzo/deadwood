//! Fixture: a `mod` declaration's `cfg` gate, and the file it names.
//!
//! Nothing here compiles — none of the crates named below exist — but all of
//! it parses, which is all Deadwood needs.

/// A test module whose body is a file of its own. The gate is written here,
/// and `src/tests.rs` holds nothing that could tell a reader of that file
/// alone that it is test code.
#[cfg(test)]
mod tests;

/// The same file under two declarations with different gates, which is the
/// corner that decides whether carrying a gate down is safe at all. Module
/// resolution reads the file once, so an answer taken from the first
/// declaration it happened to reach would be decided by queue order.
#[cfg(test)]
#[path = "both_ways.rs"]
mod gated_alias;

#[path = "both_ways.rs"]
mod ungated_alias;

/// An ungated declaration of a file that gates itself with an inner
/// `#![cfg(test)]`, which the declarations *inside* that file inherit.
mod inner_gated;

fn build() -> u32 {
    7
}

/// Written inline, the same gate needs nothing from module resolution: the
/// item walk in `src/deps.rs` sees it where it is. Its own file-backed child
/// does need it, one directory deeper.
#[cfg(test)]
mod inline_tests {
    mod helper;

    use inline_test_crate::assert_ok;

    #[test]
    fn builds_a_thing() {
        assert_ok(super::build());
    }
}
