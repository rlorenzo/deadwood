//! Behind a `pub(crate) use`, which no consumer can go through.

/// Reported: `pub(crate) use guarded::locked;` names this module outward to
/// nobody, so its only referrer being dead is evidence exactly as it was
/// before. This is the conservatism half — the new edge must read the
/// re-export's visibility, not merely that it names a module.
pub fn still_reported() -> u32 {
    5
}
