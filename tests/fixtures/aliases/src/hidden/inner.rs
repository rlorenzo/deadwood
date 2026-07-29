//! The targets `hidden`'s aliases name — one per namespace answer.

pub mod sub;

/// Braced: the type namespace alone.
pub struct Braced {
    pub field: usize,
}

/// Unit: a constructor value of its own name as well as a type.
pub struct Sole;

/// The value namespace alone.
pub fn plain() -> usize {
    0
}

/// A type and a value of one name, which compile together because Rust
/// resolves the two namespaces separately. An alias naming `Twinned` binds
/// both, and the pass has to combine them rather than pick one.
pub struct Twinned {
    pub field: usize,
}

#[allow(non_snake_case)]
pub fn Twinned() -> usize {
    0
}

/// The first leaf of the group in `hidden`: a type.
pub enum Listed {
    One,
}

/// The second leaf of the same group: a value.
pub fn tallied() -> usize {
    0
}
