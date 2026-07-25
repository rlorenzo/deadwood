//! `impl` blocks written with a qualified self type. The body can still
//! spell the type by its bare name, which is the same self-reference as the
//! header — but only when the bare name really means that type.

/// Dead: mentioned only by its own `impl`, qualified in the header and bare
/// in the body. Defined right here, so the bare name means this type without
/// a `use` — and a `use` would itself have been a reference.
pub struct Selfish;

impl crate::qualified::Selfish {
    fn build() -> Selfish {
        Selfish
    }
}

pub mod inner {
    /// Dead for the same reason, with a namesake in scope at the impl.
    pub struct Wrapper;

    /// Alive: the bare `Wrapper` inside the second impl below is *this* one,
    /// renamed by a `use`, not the type being implemented.
    pub struct Other;

    impl Other {
        pub fn make() -> Self {
            Other
        }
    }
}

use inner::Other as Wrapper;

// Qualified header again, but here the bare `Wrapper` in scope is `Other`.
// Treating it as the self-reference would hide the only use of `Other` and
// report a live item as dead.
impl crate::qualified::inner::Wrapper {
    fn build() -> Wrapper {
        Wrapper::make()
    }
}
