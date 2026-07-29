/// Nothing names this. It is reported, where `never_named` in the spliced
/// tree beside it is not, because this file is reached by a `mod` chain from
/// the crate root and so takes part in resolution like any other.
pub fn reached_both_ways() {}
