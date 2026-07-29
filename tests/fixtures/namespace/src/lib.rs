//! The definitions the namespace half of the baseline key is about.

pub mod token {
    /// The type half of syn's constructor-shim idiom, and a *braced* struct, so
    /// it binds `Group` in the type namespace alone.
    pub struct Group {
        pub span: usize,
    }

    /// The value half. It compiles beside the struct above because Rust
    /// resolves the two namespaces separately, and it shares a kind, a file, a
    /// name and a module with it — the whole of the old match key.
    ///
    /// It deliberately does not name `Group` the type, which the shim it is
    /// modelled on does: referring to it would make the struct referenced, and
    /// the fixture needs *both* halves reported so a baseline can record one.
    #[doc(hidden)]
    #[allow(non_snake_case)]
    pub fn Group(span: usize) -> usize {
        span
    }
}

/// One item written twice, once per build. Both halves are in the type
/// namespace, so nothing separates them and one baseline entry covers both —
/// which is right: it is one item, one place to open, and one fix.
#[cfg(feature = "wide")]
pub type Limb = u64;

#[cfg(not(feature = "wide"))]
pub type Limb = u32;

/// The same shape with the halves in *different* namespaces: a unit struct
/// binds a value of its own name, so it is in both, and it overlaps the
/// function opposite it. One entry covers both, and that is the answer this
/// project wants — the two cannot be compiled together (E0428), so they are two
/// spellings of one item rather than two items.
#[cfg(feature = "wide")]
pub struct Shape;

#[cfg(not(feature = "wide"))]
#[allow(non_snake_case)]
pub fn Shape() -> usize {
    0
}

/// A `mod` is in the type namespace and the `fn` below is in the value
/// namespace, so this is the same collision as `token::Group` — except that a
/// `mod` declaration is not reportable, so it produces no finding and there is
/// no key for the two to share.
pub mod parse {}

/// Reported, and the only half of the pair above that ever is.
pub fn parse() {}
