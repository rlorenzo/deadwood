//! A dead cycle: `ping` names `pong` and `pong` names `ping`, and nothing
//! names either. Both are referenced, so no reference count can ever report
//! them — not on the first run and not on the hundredth. This is the case
//! that made reachability a different question rather than a tuning of the
//! old one.
//!
//! Both are reported, one finding per definition. Each is separately
//! deletable and separately located, and a single group finding would need a
//! name for the group — which the baseline keys on, and which would move
//! whenever a member joined or left.

pub fn ping(n: u32) -> u32 {
    if n == 0 {
        0
    } else {
        pong(n - 1)
    }
}

pub fn pong(n: u32) -> u32 {
    ping(n)
}
