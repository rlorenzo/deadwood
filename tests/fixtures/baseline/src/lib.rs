//! Findings the baseline fixtures record, drift, and go stale against.

/// Referenced by nothing: an `unused_pub_item` on a line near the top of the
/// file, which is where a drifting baseline hurts.
pub fn accepted() {}

pub mod alpha {
    /// One half of a pair that shares a kind, a file *and* a name with
    /// `beta::twin`. The module is what tells them apart in the baseline key;
    /// the line is recorded and never compared.
    pub fn twin() {}
}

pub mod beta {
    pub fn twin() {}
}

// Line numbers above are load bearing: `all-baseline.json` records them as they
// are, and every other `*-baseline.json` here records them deliberately wrong.
// Add nothing above `beta` without rechecking both.

