//! `lib.rs` is a mod-rs file, so its inline blocks nest from `src/` directly.

/// No attribute on the block: children live under `src/plain/`, and the
/// `#[path]` inside it resolves from there rather than from `src/`.
pub mod plain {
    #[path = "Renamed.rs"]
    pub mod renamed;
}

/// `#[path]` on the block itself renames the directory its children live in,
/// and resolves from `src/` — the declaring file's own directory.
#[path = "builtin"]
pub mod builtins {
    #[path = "Ls.rs"]
    pub mod ls;
    /// No attribute: the stem-named lookup follows the renamed directory too.
    pub mod cat;
}

/// A `#[path]` target that is not a `mod.rs` still owns the directory it sits
/// in, so `body.rs` declares its children as *siblings* of itself.
#[path = "body.rs"]
pub mod body;

/// The nested case: a renamed block inside a renamed block.
#[path = "nested"]
pub mod outer {
    #[path = "printer"]
    pub mod inner {
        #[path = "Tree.rs"]
        pub mod tree;
    }
}

pub fn entry() -> u32 {
    plain::renamed::one()
        + builtins::ls::two()
        + builtins::cat::three()
        + body::sibling::four()
        + outer::inner::tree::five()
}
