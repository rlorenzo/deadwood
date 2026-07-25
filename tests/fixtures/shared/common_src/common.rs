pub fn shared_dead() -> u32 {
    1
}

/// Only `member_a` declares `wide`, so this gate is impossible for `member_b`
/// and perfectly ordinary for `member_a`. A file that some package compiles is
/// not dead by construction.
#[cfg(feature = "wide")]
fn only_for_a() {}
