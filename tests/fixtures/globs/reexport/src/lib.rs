//! The reproducer from
//! [#28](https://github.com/rlorenzo/deadwood/issues/28), and the shapes
//! around it that must keep their answers.
//!
//! Every module below is private, so nothing here is on the public surface by
//! the `pub`-chain rule. What puts things there is a **named `pub use` whose
//! target is a module** — the third edge, beside the glob one `facade` carries.
//!
//! The issue writes the reproducer as `mod sub; pub use sub as api;`, which
//! rustc rejects: re-exporting a module that is not itself `pub` is E0365,
//! "`sub` is only public within the crate, and cannot be re-exported outside".
//! The shape that compiles — and the one `syn` and `clap_builder` actually
//! carry — is a `pub` module under a *private ancestor*, which is what every
//! case here uses. The two spellings are then the presence or absence of a
//! rename, not the presence or absence of a path.

mod chain;
mod dead;
mod guarded;
mod item;
mod nook;

/// Spelling one, no rename: `reexport::plain::…` is public API.
pub use nook::plain;

/// Spelling two, renamed: `reexport::api::…` is public API. The rename is the
/// only difference, and neither spelling records an edge to anything — there
/// is no item to record one to.
pub use nook::renamed as api;

/// The first of two hops. `first` re-exports `second` the same way, so the
/// closure has to follow the new edge from a module the new edge itself put on
/// the surface.
pub use chain::first;

/// A named `pub use` of an *item*, which is not a surface fact and must keep
/// behaving as it does: it is a root here, and reaching it records an edge to
/// `Lifted` alone. Rooting the module `item` instead would silence everything
/// beside it.
pub use item::Lifted;

/// `pub(crate)` re-exports nothing outward, so it roots nothing however much
/// it looks like the lines above. `dead::helper` writes `crate::locked::…`
/// through it.
pub(crate) use guarded::locked;
