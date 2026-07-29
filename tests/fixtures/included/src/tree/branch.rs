// An ordinary module file reached by an ordinary `mod` declaration — the only
// thing unusual about it is that the declaration was written in a file that
// was spliced in. Its own children go back to the stem-named rule.
pub mod twig;

/// Nothing in this crate or any other names this. It is not reported as an
/// unused public item, because an `include!`-ed file takes no part in
/// resolution — see `src/modtree.rs`'s module docs.
pub fn never_named() {}

/// The one mention of `splicedep` in the crate.
pub fn uses_the_dependency() -> impl Sized {
    splicedep::thing()
}
