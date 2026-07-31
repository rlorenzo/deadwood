//! Fixture: the module tree a macro token stream declares.
//!
//! Everything here compiles: rustc expands what Deadwood only scans, so the
//! files these macros declare are genuinely part of the build — reporting
//! them dead is the false positive #60 filed, 381 findings strong in tokio.

#[macro_use]
mod machinery;

// The tokio shape: a literal `mod` inside a macro invocation's arguments
// (`cfg_fs! { pub mod fs; }`).
wrapper! {
    pub mod wrapped;
}

// The `supported_targets!` shape: the macro's rules say `mod $m` under an
// inline `mod grouped { .. }`, and the module names arrive as invocation
// idents.
emit_mods!(alpha, beta);

// The serde shape: the whole declaration lives in the macro's body, defined
// in `machinery.rs` and resolved here, at the invocation site.
tree!();
