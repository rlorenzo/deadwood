//! Under a module no `pub use` glob exports.

/// Reported: the only thing naming it is the dead re-export above, and
/// deleting that does not delete this.
pub struct Stale;
