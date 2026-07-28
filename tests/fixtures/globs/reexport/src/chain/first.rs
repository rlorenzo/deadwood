//! Hop one: on the surface only because `lib.rs` writes `pub use
//! chain::first;`.

/// Hop two, and the reason the rule is a closure rather than a pass. This
/// re-export is followed only once `first` is itself on the surface, which
/// happens by the very edge this line is another instance of. A single walk
/// over the modules in any fixed order would reach it only by luck.
///
/// Not reported either, for the same reason `Alias` in `nook::plain` is not:
/// `first` is on the surface, so a `pub use` written here is doing its job.
pub use super::second;
