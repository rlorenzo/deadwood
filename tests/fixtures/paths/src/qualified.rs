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

    /// The bare `Wrapper` inside the second impl below is *this* one, renamed
    /// by a `use`, and not the type being implemented — so a path does reach
    /// it. It is still reported, because that path is written inside an `impl`
    /// of a type nothing reaches, and the two are different findings: mistake
    /// the bare name for the impl's self-reference and this becomes the
    /// stronger "nothing names it" claim instead.
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
