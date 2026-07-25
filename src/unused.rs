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
//!   ([`UnusedItem::reexport`]), instead of being invisible.
//!
//! Whatever cannot be resolved is still treated as a use — see
//! [`crate::resolve`] for the exact fallbacks. Items carrying `#[no_mangle]`,
//! `#[used]`, `#[export_name]`, or an `allow`/`expect` for
//! `dead_code`/`unused` are skipped, as is `fn main`.
//!
//! If any file failed to parse, the symbol table would be missing both
//! definitions and the paths that use them, so the whole check is skipped for
//! that run (with a warning) instead of reporting unreliable findings.

use std::path::PathBuf;

use crate::resolve::{CrateUnit, SymbolTable};

/// A `pub` item or re-export that no path in the workspace resolves to.
pub struct UnusedItem {
    pub name: String,
    /// Item kind for display: "fn", "struct", "re-export", ...
    pub kind: &'static str,
    pub file: PathBuf,
    pub line: usize,
    /// True for a `pub use` re-export rather than a definition.
    pub reexport: bool,
}

/// Report `pub` items and `pub use` re-exports that nothing refers to.
pub fn find_unused_items(crates: &[CrateUnit], warnings: &mut Vec<String>) -> Vec<UnusedItem> {
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
        .unused_definitions()
        .into_iter()
        .map(|def| UnusedItem {
            name: def.name,
            kind: def.kind.label(),
            file: def.file,
            line: def.line,
            reexport: def.kind.is_reexport(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modtree::ParsedFile;

    /// A single-file crate whose root is `lib.rs`.
    fn crate_of(sources: &[(&str, &str)]) -> CrateUnit {
        CrateUnit {
            names: vec!["fixture".to_string()],
            files: sources
                .iter()
                .map(|(module, source)| ParsedFile {
                    path: PathBuf::from(format!("/ws/src/{module}.rs")),
                    ast: syn::parse_file(source).ok(),
                    module: if module.is_empty() {
                        Vec::new()
                    } else {
                        vec![(*module).to_string()]
                    },
                })
                .collect(),
        }
    }

    fn unused_names(crates: &[CrateUnit]) -> Vec<String> {
        let mut warnings = Vec::new();
        let found = find_unused_items(crates, &mut warnings);
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

    #[test]
    fn check_is_skipped_when_a_file_cannot_be_parsed() {
        let unit = crate_of(&[("", "pub fn dead() {}\n"), ("broken", "fn oops( {\n")]);
        let mut warnings = Vec::new();
        let unused = find_unused_items(&[unit], &mut warnings);
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
            ("", "mod a;\nmod b;\nfn go() { a::helper(); }\n"),
            ("a", "pub fn helper() {}\n"),
            ("b", "pub fn helper() {}\n"),
        ]);
        let unused = {
            let mut warnings = Vec::new();
            let found = find_unused_items(&[unit], &mut warnings);
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

    #[test]
    fn impl_generic_arguments_and_traits_are_uses() {
        let unit = crate_of(&[(
            "",
            "pub struct Held;\npub trait Marker {}\npub struct Holder<T>(T);\n\
             impl Marker for Holder<Held> {}\n",
        )]);
        assert_eq!(unused_names(&[unit]), vec!["Holder"]);
    }

    #[test]
    fn unused_reexport_is_reported_and_used_one_is_not() {
        let unit = crate_of(&[
            (
                "",
                "mod inner;\npub use inner::Used;\npub use inner::Unused;\nfn go() -> Used { Used }\n",
            ),
            ("inner", "pub struct Used;\npub struct Unused;\n"),
        ]);
        let mut warnings = Vec::new();
        let found = find_unused_items(&[unit], &mut warnings);
        assert!(warnings.is_empty());
        let reexports: Vec<_> = found
            .iter()
            .filter(|item| item.reexport)
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(reexports, vec!["Unused"]);
    }

    #[test]
    fn renamed_import_marks_the_original_used() {
        let unit = crate_of(&[
            (
                "",
                "mod inner;\nuse inner::original as renamed;\nfn go() { renamed(); }\n",
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
            }],
        };
        assert_eq!(unused_names(&[library, consumer]), vec!["unexported"]);
    }
}
