pub mod features {
    // Reached from `app` as `analytics::Features::tracked_inc()`.
    pub fn tracked_inc() {}
}

/// Re-export under the `analytics` name, as bun_core does.
pub mod analytics {
    pub use super::features as Features;
}

// Named by nothing anywhere: the control that stays reported.
pub fn orphan() {}
