//! The crate's declared public surface.

/// Referenced by nothing in the workspace, which is the normal state of an
/// exported item and indistinguishable from a dead one.
pub fn public_entry() {}

/// Called only from `generated.rs`. If ignoring that file dropped its
/// references instead of only its findings, this would turn into a
/// false-positive unused-pub finding — the thing `ignore` must never do.
pub fn helper() {}

/// Referenced from the `app` binary, so it is never a finding under any
/// configuration: the control that proves the assertions are not vacuous.
pub struct Handle;
