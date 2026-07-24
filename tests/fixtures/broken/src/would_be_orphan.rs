// Not declared by any visible `mod`, but the unparsable broken_mod.rs could
// be the one declaring it, so it must NOT be reported as dead.
pub fn maybe_used() -> u32 {
    3
}
