//! One file, three `mod` declarations in `src/lib.rs`: a `#[cfg(test)]` one
//! either side of an ungated one, all naming this file.
//!
//! The ungated declaration is what decides. This is library code however the
//! other two are gated, and so is `shared_view/deeper.rs` below it — a file
//! reached by one gated declaration and one ungated one must not be attributed
//! to the tests, or the crates it names look like dev-dependencies while the
//! library is genuinely using them.

mod deeper;

fn view() -> shared_view_crate::View {
    deeper::describe();
    shared_view_crate::View
}
