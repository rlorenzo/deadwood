//! Nothing reaches this module, which is what makes `from_glob` a cascade
//! finding rather than an unreferenced one.

/// Reported: nothing names it.
pub fn caller() -> u32 {
    crate::hidden::from_glob()
}
