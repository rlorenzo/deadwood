//! The two targets `shared`'s aliases name, one per answer.

/// Braced, so the type namespace alone.
pub struct Braced {
    pub field: usize,
}

/// Unit, so a constructor value of its own name as well as a type.
pub struct Sole;
