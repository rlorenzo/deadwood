//! One dead re-export per answer the resolving pass can give.
//!
//! Nothing names any of these, so every one is reported and prints the
//! namespace it recorded: the table this phase adds, in the output rather than
//! only in a unit test.

mod inner;
mod veiled;

/// A braced struct is in the type namespace alone, and so is an alias to it.
pub use inner::Braced;

/// The same target under another name. A rename changes what the alias is
/// called, never what it binds.
pub use inner::Braced as Renamed;

/// A unit struct binds a constructor value of its own name, so this alias
/// genuinely is in both namespaces and keeps saying so.
pub use inner::Sole;

/// A `fn` is in the value namespace alone.
pub use inner::plain;

/// A module. There is no item definition to read a namespace off — but a `mod`
/// declaration *is* a definition, in the type namespace, so the ordinary rule
/// answers it and there is no second route for the two answers to drift apart.
pub use inner::sub;

/// A name binding a type *and* a value resolves to two definitions, and an
/// alias to it binds both halves. Following only one of them would record a
/// narrow namespace for an alias that is genuinely broad, which is the one
/// direction that un-baselines a finding already accepted.
pub use inner::Twinned;

/// A group is not one question. It is split into a definition per leaf before
/// anything is resolved, so each leaf answers for itself: `Listed` is a type
/// and `tallied` is a value.
pub use inner::{Listed, tallied};

/// Refusal, with a name: the target is outside the workspace, and nothing in
/// the table can say what it is.
pub use core::fmt::Debug as Outside;

/// Refusal, with a name: the only thing that could bring `Formatter` into
/// `veiled` is a glob leading out of the workspace, so the target may be an
/// item that was never indexed.
pub use veiled::Formatter as Veiled;
