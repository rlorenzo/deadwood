//! Stands in for machine-generated code: declared by `lib.rs`, so it is a live
//! module, and covered by the `ignore` pattern, so no finding may be reported
//! about it.
//!
//! It also calls `api::helper`, which nothing else does. That call has to keep
//! counting while this file is ignored.

/// Unused, and reported in the baseline run; silenced by `ignore`.
pub fn generated_thing() {}

/// Same, and the only reference to `api::helper` anywhere.
pub fn call_helper() {
    crate::api::helper();
}
