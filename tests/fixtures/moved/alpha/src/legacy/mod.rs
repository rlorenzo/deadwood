//! `crate::legacy`, written as a directory module.
//!
//! Every `*-baseline.json` beside this fixture records the finding below at
//! `alpha/src/legacy.rs` — the file module this was converted from, which is
//! the one shape of move the module path survives exactly. The item did not
//! change; only the file it is written in did.

/// Referenced by nothing: an `unused_pub_item` in `crate::legacy`.
pub fn gone() {}
