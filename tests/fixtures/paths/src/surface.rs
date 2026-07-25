//! A re-export surface: one name is reached through this module, two are not.

mod hidden;

/// Used: `surface::Exposed` in the crate root resolves through here.
pub use hidden::Exposed;

/// Dead re-export: nothing refers to `surface::Ignored`.
pub use hidden::Ignored;

/// Dead renamed re-export.
pub use hidden::Renamed as Alias;
