//! The shape that made this rule necessary, and it is not hypothetical:
//! `winnow`'s documented `combinator::iterator` is `pub use self::core::*;`
//! over a *private* `mod core`, and the first run of this kind called it
//! test-only.
//!
//! A glob binds no name, so it records no edge the way a named `pub use` does,
//! and the root set cannot see it. Nothing here is reported.

mod inner;

pub use inner::*;
