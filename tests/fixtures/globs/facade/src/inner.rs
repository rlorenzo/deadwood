//! Private, and on the public surface all the same: `pub use inner::*;` in
//! `lib.rs` is what a consumer writes `facade::thing` through.

pub(crate) mod deep;
pub mod nested;

/// Not reported. Its only referrer in this workspace is dead, and that is no
/// evidence about an item a consumer can name — this is the false positive
/// #25 was filed for.
pub fn thing() -> u32 {
    1
}

/// Still reported, and this is the half rooting must not move: a root is not
/// exempt from "nothing names it". Silencing this would silence a library's
/// entire surface rather than the handful of cascade findings behind a glob.
pub fn never_named() -> u32 {
    2
}

/// Not reported: a consumer writes `facade::Carried`, so this `pub use` is
/// doing its job by existing, exactly as one in `lib.rs` would be.
pub use deep::Carried;
