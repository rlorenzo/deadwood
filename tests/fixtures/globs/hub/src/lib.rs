//! A cross-crate glob re-export. It roots nothing the surface rule did not
//! already cover: the only modules of another workspace member a path can name
//! are `pub` from that member's own crate root, which is the surface already —
//! so `facade`'s private `imported` and `inner::deep` keep their findings.

pub use facade::*;
