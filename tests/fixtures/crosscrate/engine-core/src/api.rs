/// Used from `app` through a cross-crate import.
pub struct Handle;

/// Dead: `app` imports `Handle` and nothing else from here.
pub struct Unused;
