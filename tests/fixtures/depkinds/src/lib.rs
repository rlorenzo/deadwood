//! Fixture: which manifest table each entry belongs in.
//!
//! Nothing here compiles — none of the crates named below exist — but all of
//! it parses, which is all Deadwood needs. Every entry is named exactly once,
//! from exactly one kind of code, so each one pins a different answer.

use shared_crate::Thing;

// The `serde_json` shape, and the reason this fixture has two crates spelled
// almost alike. The rename binds for this crate only, so every
// `aliased_crate::` below is a mention of `renamed_core_crate` — not of the
// `[dev-dependencies] aliased_crate` entry, which `tests/it.rs` names for
// real. The declaration itself names `renamed_core_crate`, which is why the
// unused check never calls that entry dead.
extern crate renamed_core_crate as aliased_crate;

// The edition-2018 spelling of the same thing, which binds a crate name just
// as the line above does. `use real::item as alias;` would not: that renames
// an item, and its head is still the crate.
use use_renamed_crate as use_aliased_crate;

// The boundary. This renames an *item*, not a crate: the head is still
// `shared_crate`, and `cfg_test_crate` here is a type in this module rather
// than a crate name. Treating it as a crate rename would fold the
// dev-dependency of that name onto `shared_crate` and report it unused.
use shared_crate::Thing as cfg_test_crate;

// An alias inside a nested module binds inside it and nowhere else. The
// mention below is `nested_renamed_crate`; the one at the crate root, further
// down, is the `nested_alias_crate` dependency itself.
mod nested_extern {
    // The same scoping question through the other spelling.
    extern crate nested_extern_renamed as nested_extern_alias;
    fn f() {
        nested_extern_alias::helper();
    }
}

// A crate-root `use` rename binds in this module only. `src/crossfile.rs` is a
// different module and sees the crate itself, so what it names is the
// `crossfile_alias_crate` dependency and not this rename.
use crossfile_renamed_crate as crossfile_alias_crate;
mod crossfile;

mod nested {
    use nested_renamed_crate as nested_alias_crate;
    // Private: what confines a mention is where it is written, not what it is
    // visible to, and keeping this off the public surface keeps the fixture's
    // findings to the dependency tables it is about.
    fn f() {
        nested_alias_crate::helper();
    }
}

/// Builds a thing, on top of doc_and_library_dev_crate.
///
/// That name in this sentence is an opaque mention, and the body below names
/// the same crate in code. One opaque mention stops the entry being judged at
/// all, so no placement claim is made about it in either direction.
///
/// The example is compiled as a doctest, which links the dev-dependencies, so
/// what it names is correctly declared as one:
///
/// ```
/// use doc_only_crate::harness;
/// harness().check();
/// ```
pub fn build() -> Thing {
    stale_build_crate::helper();
    // Reads as `aliased_crate` and means `renamed_core_crate`.
    aliased_crate::helper();
    use_aliased_crate::helper();
    // Out here the alias above does not apply: this is the crate itself.
    nested_alias_crate::helper();
    nested_extern_alias::helper();
    crossfile_alias_crate::helper();
    // The crate root, where `src/crossfile.rs`'s rename does not reach.
    modfile_alias_crate::helper();
    // Two `[dev-dependencies]` entries named by the library itself, which is a
    // build that cannot link them. The second is named by `tests/it.rs` too,
    // and is reported all the same: one runtime mention decides it.
    library_named_dev_crate::helper();
    library_and_test_dev_crate::helper();
    doc_and_library_dev_crate::helper();
    Thing
}

/// A mention Deadwood cannot see through. The body is not expanded, so
/// `opaque_dev_crate` is known to be used and not known to be used *where* —
/// which keeps the entry off both placement claims.
macro_rules! opaque_use {
    () => {
        opaque_dev_crate::helper()
    };
}
pub(crate) use opaque_use;

/// The same unit tests written out of line: the gate is here, the code is in
/// `src/outline_tests.rs`, and nothing in that file says what it is.
#[cfg(test)]
mod outline_tests;

// One file under three declarations. The gated ones sit either side of the
// ungated one so that neither pop order can decide the answer: whichever
// declaration is read first, the ungated one is what this file is.
#[cfg(test)]
#[path = "shared_view.rs"]
mod view_before_the_ungated_one;

#[path = "shared_view.rs"]
mod shared_view;

#[cfg(test)]
#[path = "shared_view.rs"]
mod view_after_the_ungated_one;

/// Unit tests live inside the library target and still link the
/// dev-dependencies, which is what makes this the check's hardest case.
#[cfg(test)]
mod tests {
    use cfg_test_crate::assert_ok;

    #[test]
    fn builds_a_thing() {
        assert_ok(super::build());
        // A bare `#[test]` inside a module `#[cfg(test)]` already confined.
        // The module moved this code once; the attribute must not be a second,
        // separate answer about the same mention.
        nested_test_fn_crate::assert_ok();
    }
}

// A bare `#[test] fn` at module scope, with no `#[cfg(test)]` anywhere near it.
// This is how `clap_builder` writes `check_auto_traits`, and rustc leaves the
// function out of every build that is not a test build — verified: the same
// file compiles as a library and fails under `--test` when the crate it names
// does not exist.
#[test]
fn checks_a_thing() {
    // A `[dev-dependencies]` entry, correctly declared. Reporting it would be
    // a finding invented against a manifest that compiles.
    test_fn_dev_crate::assert_ok();
    // A `[dependencies]` entry no other code names: it belongs one table down,
    // and this is the finding the gap used to cost.
    test_fn_crate::assert_ok();
}

#[bench]
fn benches_a_thing() {
    bench_fn_crate::assert_ok();
}

// The boundary between a built-in attribute that confines (`#[test]`, above)
// and one that does not: `#[should_panic]` on its own confines nothing, rustc
// compiles the function into the library, and the mention is library code.
#[should_panic]
fn panics_on_a_thing() {
    should_panic_crate::assert_ok();
}

// An attribute macro Deadwood cannot expand, on a `[dependencies]` entry's
// only mention. The item is the macro's input, so the mention is opaque: known
// used, unknown where. The entry stays put either way — this pins that the
// opacity does not invent a downgrade claim.
#[harness::test]
fn drives_a_thing() {
    proc_macro_test_crate::assert_ok();
}

// The `#[tokio::test]` shape issue #49 filed, split across two entries: the
// macro's own crate is a `[dependencies]` entry plain library code names, and
// the function it owns holds the only mention of a `[dev-dependencies]`
// entry. Before the item was macro input, that mention read as library code
// and the entry was reported as belonging in `[dependencies]` — against a
// manifest that compiles. Private, like everything here that exists only to
// place a mention: what confines a mention is where it is written, and this
// keeps the fixture's findings to the dependency tables it is about.
fn hosts_a_macro() {
    attr_macro_host_crate::runtime();
}

#[attr_macro_host_crate::test]
fn drives_an_async_thing() {
    attr_macro_dev_crate::assert_ok();
}

// The single-segment spelling of the same thing. `imported_attr` is no
// built-in attribute and there is no `#[derive]` here for it to be a helper
// of, so on stable rustc it can only be an attribute macro in scope.
use attr_macro_host_crate::imported_attr;

#[imported_attr]
fn drives_through_an_import() {
    single_segment_attr_dev_crate::assert_ok();
}

// The spelling the built-in attribute expands to. rustc honours it — and a
// proc macro can imitate it, which is why phase 20 refused to match it as
// `#[test]`. Opaque closes the same gap from the other side.
#[core::prelude::v1::test]
fn drives_the_expanded_spelling() {
    core_prelude_test_dev_crate::assert_ok();
}

// An attribute macro on an associated fn. `#[test]` could never confine one
// (`Site::Other`), but macro ownership has no site: the `impl` member is the
// macro's input all the same.
struct Instrumented;

impl Instrumented {
    #[attr_macro_host_crate::instrument]
    fn measures_a_thing(&self) {
        attr_macro_impl_dev_crate::assert_ok();
    }
}

// A derive helper attribute is not an attribute macro, however unknown its
// name: it belongs to the `#[derive]` beside it and rewrites nothing. The
// field type is a library mention of a `[dev-dependencies]` entry, and it must
// stay one — sweeping helpers into opacity would silence the finding.
#[derive(FakeSerialize)]
#[fake_helper(compact)]
struct Configured {
    #[fake_helper(rename = "w")]
    widget: helper_attr_dev_crate::Widget,
}

// A tool attribute is metadata for the tool whose namespace it names, never a
// macro. The corpus spells `#[rustfmt::skip]` freely in library code, so this
// staying attributable is what keeps the check's recall there.
#[rustfmt::skip]
fn formats_a_thing() {
    tool_attr_dev_crate::assert_ok();
}

// A built-in attribute rewrites nothing, whatever it does to codegen.
#[inline]
fn inlines_a_thing() {
    builtin_attr_dev_crate::assert_ok();
}
