//! Findings the baseline fixtures record, drift, and go stale against.

/// Referenced by nothing: an `unused_pub_item` on a line near the top of the
/// file, which is where a drifting baseline hurts.
pub fn accepted() {}

pub mod alpha {
    /// One half of a pair that shares a kind, a file *and* a name with
    /// `beta::twin`. The line is the only thing telling them apart, and the
    /// baseline key does not look at it.
    pub fn twin() {}
}

pub mod beta {
    pub fn twin() {}
}
