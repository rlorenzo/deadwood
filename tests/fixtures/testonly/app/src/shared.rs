//! The other half of the claim: an item a non-test entry point also reaches is
//! reached by a build with no tests in it, so it is not test-only.

/// Not reported: `main` calls it too.
pub fn both() -> u32 {
    3
}

#[cfg(test)]
mod tests {
    #[test]
    fn covers_it() {
        if super::both() != 3 {
            panic!("both is broken");
        }
    }
}
