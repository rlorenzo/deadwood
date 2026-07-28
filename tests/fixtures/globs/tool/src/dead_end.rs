//! Nothing reaches this module, which is what makes `from_glob` and
//! `from_named_reexport` cascade findings rather than unreferenced ones.

/// Reported: nothing names it.
pub fn caller() -> u32 {
    crate::hidden::from_glob() + crate::tucked::inner::from_named_reexport()
}
