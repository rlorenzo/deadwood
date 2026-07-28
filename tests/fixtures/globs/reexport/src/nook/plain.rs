//! On the public surface because `lib.rs` writes `pub use nook::plain;` —
//! the spelling with no rename.

/// Not reported. Its only referrer in this workspace is dead, and that is no
/// evidence about an item a consumer names as
/// `reexport::plain::only_dead_names_it` — this is the false positive #28 was
/// filed for.
pub fn only_dead_names_it() -> u32 {
    1
}

/// Still reported, and this is the half rooting must not move: a root is not
/// exempt from "nothing names it". It is also the shape `syn` carries — its
/// one finding sits inside `crate::gen::visit_mut`, one of the modules this
/// rule newly reaches, and is a first-condition finding no surface rule can
/// touch.
pub fn nothing_names_it() -> u32 {
    2
}

/// Not reported: a `pub use` in a module on the public surface is doing its job
/// by existing, and this module is on the surface *only* by the new edge. The
/// root set and the re-export filter read one set, so widening it for one
/// widens it for the other.
pub use crate::item::Lifted as Alias;
