//! One file, three `mod` declarations in `src/lib.rs`: a `#[cfg(test)]` one
//! either side of an ungated one, all naming this file.
//!
//! The ungated declaration is what decides. This is library code however the
//! other two are gated, and so is `shared_view/deeper.rs` below it — a file
//! reached by one gated declaration and one ungated one must not be attributed
//! to the tests, or the crates it names look like dev-dependencies while the
//! library is genuinely using them.

mod deeper;

// The same case as `deeper` above, reached the other way: this file is walked
// once under a gated declaration, and the ungated one lifts it. What a walk
// splices in has to be lifted with what it declares.
include!("shared_view/spliced.rs");

/// An *inline* module in a file three declarations reach, which is the case
/// the inline list has to be replaced rather than added to. Whichever gated
/// declaration is read first records this module as confined, because the file
/// it sits in was; the ungated declaration then lifts the file, and the list
/// is recomputed from the module's own gate — which is no gate at all.
mod inline_view {
    fn describe() {}
}

fn view() -> shared_view_crate::View {
    deeper::describe();
    shared_view_crate::View
}
