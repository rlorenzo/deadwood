/// Used from the crate root through a renamed import.
pub fn shared() -> u32 {
    1
}

/// Shares its name with `beta::collision`, and is the one actually called.
pub fn collision() -> u32 {
    2
}

/// Dead: the only mentions of this type are in its own `impl` block, which
/// says nothing about anyone using it.
pub struct ImplOnly;

impl ImplOnly {
    pub fn make() -> Self {
        ImplOnly
    }
}
