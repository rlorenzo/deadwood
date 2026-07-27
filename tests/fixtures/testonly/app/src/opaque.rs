//! An opaque mention is a root in *both* walks, so an item one names can never
//! be test-only however test-only it looks.
//!
//! This is the conservative direction — a mention Deadwood has admitted it
//! cannot read must not become evidence — and it is by far the most expensive
//! rule in the kind: `assert_eq!` is how most tests name what they are
//! testing, and one of them is enough.

/// Not reported, though only the test below reaches it.
pub fn mentioned() -> u32 {
    2
}

#[cfg(test)]
mod tests {
    #[test]
    fn covers_it() {
        assert_eq!(super::mentioned(), 2);
    }
}
