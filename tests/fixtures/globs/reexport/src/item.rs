//! The case that needed no fixing, and the one a wider rule would break.

/// Not reported: `pub use item::Lifted;` in `lib.rs` is a definition of its
/// own, it is a root there, and reaching it records an edge to this. That
/// route predates #28 and is untouched by it.
pub struct Lifted;

/// Reported: a named `pub use` of an *item* carries that item out and nothing
/// else. Reading it as a surface fact about `item` would root this too, and
/// its only referrer is dead.
pub fn beside_the_lifted_one() -> u32 {
    6
}
