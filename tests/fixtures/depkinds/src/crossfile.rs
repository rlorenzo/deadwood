//! A submodule file, which the crate root's `use ... as ...` does not reach.
//!
//! `crossfile_alias_crate` here is the dependency of that name, resolved
//! through the extern prelude — not the rename written in `src/lib.rs`.

// Not the crate root, so this is an ordinary item of module `crossfile` and
// binds only here — unlike the same line in `src/lib.rs`, which would enter the
// extern prelude and hold for every module.
extern crate modfile_renamed_crate as modfile_alias_crate;

pub(crate) fn f() {
    crossfile_alias_crate::helper();
    modfile_alias_crate::helper();
}
