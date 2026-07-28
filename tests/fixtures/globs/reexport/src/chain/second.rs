//! Two hops in: nothing names `chain`, and nothing names `second` from
//! outside except through `first`.

/// Not reported: a consumer names it
/// `reexport::first::second::two_hops_from_the_root`.
pub fn two_hops_from_the_root() -> u32 {
    4
}
