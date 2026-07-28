//! A second member with a `crate::legacy` of its own.
//!
//! `migrated` occurs here and nowhere else, so an entry recording it against a
//! file in `alpha` has exactly one candidate to pair with — and the module path
//! matches, because a module path says nothing about which crate it is in. The
//! package is what refuses the pairing.

mod legacy {
    /// Referenced by nothing: an `unused_pub_item` in `crate::legacy`, in
    /// `beta` rather than `alpha`.
    pub fn migrated() {}
}
