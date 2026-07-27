//! Unused `pub` item and re-export detection.
//!
//! Rustc's `dead_code` lint already covers private and crate-visible items,
//! but it assumes fully-`pub` items are used, because it cannot see the
//! consumers. Within a workspace we can do better: a `pub` item that no path
//! anywhere in the workspace resolves to is either dead or external API.
//! Deadwood reports it and lets the author decide.
//!
//! Usage comes from [`crate::resolve`], which resolves `use` declarations and
//! qualified paths against a per-crate symbol table instead of counting bare
//! identifiers. Consequences worth knowing:
//!
//! - Two items sharing a name no longer hide each other; each is judged on
//!   the paths that actually reach it.
//! - A type's own `impl` blocks are not uses of it, so a struct that only
//!   ever appears in `impl Struct { ... }` is reported.
//! - A `pub use` re-export nothing goes through is reported in its own right
//!   ([`UnusedItem::reexport`]), instead of being invisible — unless it sits
//!   on a library's public surface, where having no workspace-internal user
//!   is the point rather than a defect.
//! - Being referenced is not enough: the referrer has to be alive too
//!   ([`UnusedItem::only_from_unreached`]). An item a dead subsystem calls is
//!   dead, and so is a pair of mutually recursive functions nothing reaches —
//!   the case a reference count cannot express at all.
//!
//! Whatever cannot be resolved is still treated as a use — and, under
//! reachability, as an unconditional one; see [`crate::resolve`] for the exact
//! fallbacks and for what counts as a root. Items carrying `#[no_mangle]`,
//! `#[used]`, `#[export_name]`, or an `allow`/`expect` for
//! `dead_code`/`unused` are skipped, as is `fn main`.
//!
//! The remaining gap is one Deadwood cannot close by looking harder: a `pub`
//! item consumed by a crate *outside* the workspace is indistinguishable from
//! a dead one, which is why these findings are advisory for libraries. The
//! `public-api` setting in [`crate::config`] is how a project closes it, by
//! declaring which crates and item paths are surface rather than leftovers.
//!
//! If any file failed to parse, the symbol table would be missing both
//! definitions and the paths that use them, so the whole check is skipped for
//! that run (with a warning) instead of reporting unreliable findings.

use std::path::PathBuf;

use crate::config::PublicApi;
use crate::resolve::{CrateUnit, SymbolTable};

/// A `pub` item or re-export that nothing live in the workspace reaches.
pub struct UnusedItem {
    pub name: String,
    /// Item kind for display: "fn", "struct", "re-export", ...
    pub kind: &'static str,
    pub file: PathBuf,
    pub line: usize,
    /// True for a `pub use` re-export rather than a definition.
    pub reexport: bool,
    /// Whether paths do resolve to this item and every one of them is written
    /// inside something nothing reaches. The two are one finding kind because
    /// they are one claim — the item is dead — but they are not the same
    /// evidence, and a message saying "never referenced" about an item with
    /// callers reads as a bug in the tool.
    pub only_from_unreached: bool,
}

/// Report `pub` items and `pub use` re-exports that nothing refers to,
/// excluding whatever `public_api` declares to be external surface.
pub fn find_unused_items(
    crates: &[CrateUnit],
    public_api: &PublicApi,
    warnings: &mut Vec<String>,
) -> Vec<UnusedItem> {
    // Resolution must see every file: a file that failed to parse hides both
    // definitions and the paths that reach them, and missing paths turn into
    // false positives.
    if crates
        .iter()
        .any(|unit| unit.files.iter().any(|file| file.ast.is_none()))
    {
        warnings.push(
            "unused-pub check skipped: usage resolution would be unreliable with unparsable files"
                .to_string(),
        );
        return Vec::new();
    }

    let mut table = SymbolTable::build(crates);
    table.record_references(crates);
    table
        .unused_definitions(public_api)
        .into_iter()
        .map(|def| UnusedItem {
            name: def.name,
            kind: def.kind.label(),
            file: def.file,
            line: def.line,
            reexport: def.kind.is_reexport(),
            only_from_unreached: def.only_from_unreached,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modtree::ParsedFile;

    /// A library crate from `(module path, source)` pairs, where the module
    /// path is `/`-separated and empty for the crate root.
    fn crate_of(sources: &[(&str, &str)]) -> CrateUnit {
        CrateUnit {
            names: vec!["fixture".to_string()],
            files: sources
                .iter()
                .map(|(module, source)| ParsedFile {
                    path: PathBuf::from(format!("/ws/src/{module}.rs")),
                    ast: syn::parse_file(source).ok(),
                    module: module
                        .split('/')
                        .filter(|segment| !segment.is_empty())
                        .map(str::to_string)
                        .collect(),
                    test_only: false,
                })
                .collect(),
        }
    }

    fn unused_names(crates: &[CrateUnit]) -> Vec<String> {
        let mut warnings = Vec::new();
        let found = find_unused_items(crates, &PublicApi::default(), &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        found.into_iter().map(|item| item.name).collect()
    }

    #[test]
    fn flags_unreferenced_pub_fn_only() {
        let unit = crate_of(&[(
            "",
            "pub fn dead() {}\npub fn alive() {}\nfn caller() { alive(); }\n",
        )]);
        assert_eq!(unused_names(&[unit]), vec!["dead"]);
    }

    #[test]
    fn usage_inside_macro_body_counts() {
        let unit = crate_of(&[(
            "",
            "pub fn helper() {}\nfn go() { println!(\"{}\", helper as usize); }\n",
        )]);
        assert!(unused_names(&[unit]).is_empty());
    }

    #[test]
    fn skips_allow_dead_code_and_main() {
        let unit = crate_of(&[(
            "",
            "#[allow(dead_code)]\npub fn kept() {}\npub fn main() {}\n",
        )]);
        assert!(unused_names(&[unit]).is_empty());
    }

    /// Edition 2024 spells the linker exports `#[unsafe(no_mangle)]`, which
    /// parses as an attribute named `unsafe` holding the real one. Reading the
    /// outer path alone reports every export written the way current Rust
    /// requires.
    #[test]
    fn skips_an_export_wrapped_in_the_unsafe_attribute() {
        let unit = crate_of(&[(
            "",
            "#[unsafe(no_mangle)]\npub extern \"C\" fn exported() {}\n\
             #[unsafe(export_name = \"renamed\")]\npub fn named() {}\n",
        )]);
        assert!(unused_names(&[unit]).is_empty());
    }

    #[test]
    fn check_is_skipped_when_a_file_cannot_be_parsed() {
        let unit = crate_of(&[("", "pub fn dead() {}\n"), ("broken", "fn oops( {\n")]);
        let mut warnings = Vec::new();
        let unused = find_unused_items(&[unit], &PublicApi::default(), &mut warnings);
        assert!(
            unused.is_empty(),
            "incomplete resolution must not report findings"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unused-pub check skipped")),
            "the skip must be surfaced as a warning: {warnings:?}"
        );
    }

    #[test]
    fn non_pub_items_are_ignored() {
        let unit = crate_of(&[("", "pub(crate) fn crate_only() {}\nfn private() {}\n")]);
        assert!(unused_names(&[unit]).is_empty());
    }

    #[test]
    fn same_name_in_another_module_does_not_count_as_a_use() {
        // The old identifier census could not tell these apart and stayed
        // quiet about both.
        let unit = crate_of(&[
            ("", "mod a;\nmod b;\n#[test]\nfn go() { a::helper(); }\n"),
            ("a", "pub fn helper() {}\n"),
            ("b", "pub fn helper() {}\n"),
        ]);
        let unused = {
            let mut warnings = Vec::new();
            let found = find_unused_items(&[unit], &PublicApi::default(), &mut warnings);
            assert!(warnings.is_empty());
            found
        };
        assert_eq!(unused.len(), 1, "only b::helper is dead");
        assert_eq!(unused[0].name, "helper");
        assert_eq!(unused[0].file, PathBuf::from("/ws/src/b.rs"));
    }

    #[test]
    fn a_types_own_impl_block_is_not_a_use() {
        let unit = crate_of(&[(
            "",
            "pub struct Lonely;\nimpl Lonely { pub fn new() -> Self { Lonely } }\n",
        )]);
        assert_eq!(unused_names(&[unit]), vec!["Lonely"]);
    }

    /// The self type of a qualified `impl` is still not a use, but its head
    /// segment is a path qualifier, not a name: suppressing it inside the
    /// body would hide every `crate::` path written there.
    #[test]
    fn a_qualified_impl_header_does_not_suppress_paths_in_its_body() {
        let unit = crate_of(&[
            (
                "",
                "mod thing;\nmod other;\n\
                 impl crate::thing::Wrapper { fn go() { crate::other::called(); } }\n",
            ),
            ("thing", "pub struct Wrapper;\n"),
            ("other", "pub fn called() {}\n"),
        ]);
        let mut warnings = Vec::new();
        let found = find_unused_items(&[unit], &PublicApi::default(), &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        // `called` comes out with `Wrapper` because the only thing naming it
        // is an `impl` of a type nothing reaches, which is reachability doing
        // its job rather than the header suppressing anything.
        let names: Vec<&str> = found.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["called", "Wrapper"]);
        let called = found
            .iter()
            .find(|item| item.name == "called")
            .expect("`called` is reported");
        assert!(
            called.only_from_unreached,
            "the body path resolved to `called`; had the header suppressed it the finding \
             would be the stronger `nothing names it` one"
        );
    }

    #[test]
    fn attribute_arguments_name_items_even_inside_strings() {
        // `#[serde(with = "...")]` and friends name real items in a form only
        // the deriving macro understands.
        let unit = crate_of(&[
            (
                "",
                "mod codec;\npub struct Wire {\n    #[serde(with = \"crate::codec\")]\n    field: u8,\n}\n",
            ),
            ("codec", "pub fn serialize() {}\n"),
        ]);
        assert_eq!(unused_names(&[unit]), vec!["Wire"]);
    }

    #[test]
    fn impl_generic_arguments_and_traits_are_uses() {
        let unit = crate_of(&[(
            "",
            "pub struct Held;\npub trait Marker {}\npub struct Holder<T>(T);\n\
             impl Marker for Holder<Held> {}\n",
        )]);
        assert_eq!(unused_names(&[unit]), vec!["Holder"]);
    }

    fn unused_reexports(crates: &[CrateUnit]) -> Vec<String> {
        let mut warnings = Vec::new();
        let found = find_unused_items(crates, &PublicApi::default(), &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        found
            .into_iter()
            .filter(|item| item.reexport)
            .map(|item| item.name)
            .collect()
    }

    /// Inside a private module, a `pub use` nothing goes through is dead with
    /// certainty: no code outside the workspace can reach it either.
    #[test]
    fn unused_reexport_out_of_public_reach_is_reported() {
        let unit = crate_of(&[
            (
                "",
                "mod wrapper;\n#[test]\nfn go() -> wrapper::Used { wrapper::Used }\n",
            ),
            (
                "wrapper",
                "mod inner;\npub use inner::Used;\npub use inner::Unused;\n",
            ),
            ("wrapper/inner", "pub struct Used;\npub struct Unused;\n"),
        ]);
        assert_eq!(unused_reexports(&[unit]), vec!["Unused"]);
    }

    /// A re-export reachable from a library's crate root is the public-API
    /// idiom; nothing in the workspace going through it is its normal state,
    /// not a finding.
    #[test]
    fn reexport_on_a_librarys_public_surface_is_left_alone() {
        let unit = crate_of(&[
            ("", "pub mod facade;\n"),
            ("facade", "mod inner;\npub use inner::Exported;\n"),
            ("facade/inner", "pub struct Exported;\n"),
        ]);
        assert!(unused_reexports(&[unit]).is_empty());
    }

    /// ...but the same re-export in a target nothing can import (a bin, a
    /// test) has no external consumer to serve.
    #[test]
    fn reexport_in_a_non_library_target_is_reported() {
        let binary = CrateUnit {
            names: Vec::new(),
            files: crate_of(&[
                ("", "pub mod facade;\nfn main() {}\n"),
                ("facade", "mod inner;\npub use inner::Exported;\n"),
                ("facade/inner", "pub struct Exported;\n"),
            ])
            .files,
        };
        assert_eq!(unused_reexports(&[binary]), vec!["Exported"]);
    }

    #[test]
    fn renamed_import_marks_the_original_used() {
        let unit = crate_of(&[
            (
                "",
                "mod inner;\nuse inner::original as renamed;\n#[test]\nfn go() { renamed(); }\n",
            ),
            ("inner", "pub fn original() {}\n"),
        ]);
        assert!(unused_names(&[unit]).is_empty());
    }

    #[test]
    fn unresolvable_glob_import_keeps_everything_used() {
        // `external::*` cannot be followed, so a name that is not otherwise
        // in scope must be assumed to come from it.
        let unit = crate_of(&[
            (
                "",
                "mod inner;\nuse external_crate::*;\nfn go() { mystery(); }\n",
            ),
            ("inner", "pub fn mystery() {}\n"),
        ]);
        assert!(unused_names(&[unit]).is_empty());
    }

    #[test]
    fn cross_crate_paths_resolve_by_crate_name() {
        let library = crate_of(&[("", "pub fn exported() {}\npub fn unexported() {}\n")]);
        let consumer = CrateUnit {
            names: Vec::new(),
            files: vec![ParsedFile {
                path: PathBuf::from("/ws/src/main.rs"),
                ast: syn::parse_file("fn main() { fixture::exported(); }\n").ok(),
                module: Vec::new(),
                test_only: false,
            }],
        };
        assert_eq!(unused_names(&[library, consumer]), vec!["unexported"]);
    }
}
