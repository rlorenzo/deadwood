//! Unused dependency detection.
//!
//! A dependency declared in a package's `Cargo.toml` that the package's code
//! never refers to is dead weight: it slows builds, widens the supply-chain
//! surface, and misleads readers about what the crate actually needs. Cargo
//! never complains, because it has no reason to look.
//!
//! # How it works
//!
//! For each workspace package we gather the crate names its code could be
//! referring to — from every target, including tests, examples, benches and
//! the build script — and report the manifest entries whose name is not among
//! them. The entry is reported as the user wrote it, so a renamed dependency
//! (`motor = { package = "engine-core" }`) is reported as `motor` even though
//! nothing in the manifest spells `motor` anywhere else.
//!
//! Unlike the other detectors, this one does not care whether code is
//! *reachable*: it also reads every `.rs` file in the package directory that
//! the module tree never reached. A file can hide from `mod` resolution and
//! still be compiled — `automod::dir!("tests/regression")` expands into the
//! `mod` declarations for a whole directory, and serde_json's only use of
//! `serde_derive` lives behind exactly that. The question here is whether the
//! package's sources mention the crate at all, and a mention in a file we
//! judged dead is still a mention.
//!
//! # What counts as a reference
//!
//! Everything that could possibly be one. A crate name reaches code through
//! more channels than a `use` declaration, and most of them are invisible to
//! a syntactic tool, so the collector is deliberately over-eager:
//!
//! - the head segment of any multi-segment path, and of any `use` path;
//! - `extern crate` names;
//! - every identifier inside a macro invocation, at any nesting depth,
//!   because we do not expand macros;
//! - every identifier in an attribute, *including words inside its string
//!   arguments* — `#[serde(with = "some_crate::codec")]` names a crate in a
//!   form only the deriving macro understands;
//! - every word in a doc comment, because doc examples are compiled as
//!   doctests and routinely `use` a dependency that appears nowhere else;
//! - dependency names in the manifest's own `[features]` table (`dep:foo`,
//!   `foo/bar`), which is a use with no code behind it at all.
//!
//! The cost is missed findings — a dependency whose name happens to be a
//! common word (`log`, `time`, `bytes`) is kept alive by any mention of that
//! word. That is the trade Deadwood always makes.
//!
//! # What is skipped, and why
//!
//! - **Optional dependencies.** They are pulled in by features, and Deadwood
//!   does not evaluate `cfg(feature = ...)` yet, so whether the code that
//!   uses them is even compiled is unknown.
//! - **`[target.'cfg(...)'.dependencies]`.** Same reason, for platform gates:
//!   the code using them typically sits behind a `cfg` we do not evaluate.
//! - **Packages that pull in code we cannot read.** `include!("other.rs")`
//!   and `#![doc = include_str!("../README.md")]` are followed — the included
//!   code is walked, the included documentation is mined for words, since its
//!   examples are doctests. What cannot be followed is a file whose path is
//!   only known at build time (`include!(concat!(env!("OUT_DIR"), ..))`), and
//!   that file can hold the only reference to a dependency.
//! - **Packages whose module tree did not resolve** (a parse failure, a `mod`
//!   pointing at a missing file), handled by the caller, since a file we
//!   could not read may hold the only reference.
//!
//! Each skip is surfaced as a warning rather than guessed at.
//!
//! # Dependency kinds
//!
//! A manifest entry is judged against references from *every* target of its
//! package, not only the targets that can legitimately see it. Narrowing that
//! per kind — `[dev-dependencies]` against tests only, `[dependencies]`
//! against lib and bins only — would turn "declared in the wrong table" into
//! an unused-dependency finding, and a user staring at `cc::Build::new()` in
//! their `build.rs` would rightly call that a false positive. Whether an
//! entry sits in the right table is a different check; this one answers only
//! whether the code names it at all. The reported kind still comes from the
//! entry, so the message names the table to edit.
//!
//! This also means the acceptance-critical cases fall out directly: a
//! dev-dependency used only in `tests/`, and a build-dependency used only in
//! `build.rs`, are both seen, because those targets are scanned like any
//! other. Reporting the wrong table as its own finding is tracked in
//! <https://github.com/rlorenzo/deadwood/issues/10>.
//!
//! # Known limitations
//!
//! - A dependency declared only to turn on a feature of a *transitive*
//!   dependency (the `getrandom = { features = ["js"] }` idiom) is not
//!   referenced by any code or by this package's `[features]` table, and is
//!   reported. There is no syntactic signal that distinguishes it from a
//!   stale entry; an allowlist in the config file is the intended answer
//!   (<https://github.com/rlorenzo/deadwood/issues/9>).
//! - A dependency reachable only through a glob import of another crate's
//!   prelude (a derive macro re-exported by a facade crate) is invisible.
//! - Only the source tree in front of us counts. A crate unpacked from a
//!   published `.crate` archive usually has its `tests/` and `benches/`
//!   stripped, so the dev-dependencies those used are correctly reported as
//!   unreferenced *by that tree* — and would not be, in the repository it was
//!   published from.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;

use crate::metadata::{DependencyKind, Package};
use crate::modtree::ParsedFile;

/// Why a package's reference set is incomplete, phrased for a warning.
const INCLUDE_REASON: &str = "code is pulled in with `include!` from a file that could not be read";
const DOC_MACRO_REASON: &str =
    "documentation is generated by a macro, so the doctests in it are not analyzed";
const UNPARSABLE_REASON: &str = "a source file could not be read or parsed";

/// How deep a chain of `include!`d files is followed. Real code never nests.
const MAX_INCLUDE_DEPTH: usize = 8;

/// Every crate name a package's code could be referring to.
#[derive(Default)]
pub struct CrateReferences {
    names: HashSet<String>,
    /// Set when the package pulls in code Deadwood never sees, which would
    /// make any "never referenced" verdict a guess.
    hidden_code: Option<&'static str>,
}

impl CrateReferences {
    /// Add every crate name the files of one target refer to.
    pub fn add_target(&mut self, files: &[ParsedFile]) {
        for file in files {
            match &file.ast {
                Some(ast) => self.add_file(ast, parent_of(&file.path)),
                None => self.hidden_code = Some(UNPARSABLE_REASON),
            }
        }
    }

    /// Add every crate name mentioned by a `.rs` file under `dir` that the
    /// module tree did not already reach.
    ///
    /// `already_read` holds the files [`Self::add_target`] has covered, taken
    /// from module resolution; the rest are read and parsed here.
    pub fn add_unreached_sources(&mut self, dir: &Path, already_read: &HashSet<PathBuf>) {
        for path in package_sources(dir) {
            if already_read.contains(&path) {
                continue;
            }
            match parse(&path) {
                Some(ast) => self.add_file(&ast, parent_of(&path)),
                // Unlike module resolution, this walk is speculative: it finds
                // files nothing declares. One we cannot read may still be
                // compiled, and may hold the only reference to a dependency.
                None => self.hidden_code = Some(UNPARSABLE_REASON),
            }
        }
    }

    fn add_file(&mut self, ast: &syn::File, dir: PathBuf) {
        self.add_file_at_depth(ast, dir, 0);
    }

    fn add_file_at_depth(&mut self, ast: &syn::File, dir: PathBuf, depth: usize) {
        let mut collector = Collector {
            names: &mut self.names,
            hidden_code: &mut self.hidden_code,
            dir,
            included_code: Vec::new(),
            included_text: Vec::new(),
        };
        // Inner attributes belong to the file itself and are not reached by
        // walking its items: `#![doc = ...]` lives here.
        for attr in &ast.attrs {
            collector.visit_attribute(attr);
        }
        for item in &ast.items {
            collector.visit_item(item);
        }
        let (code, text) = (collector.included_code, collector.included_text);

        // `include!("generated.rs")` splices a file into this one, and
        // `#![doc = include_str!("../README.md")]` splices in documentation
        // whose examples become doctests. Both are ordinary files most of the
        // time, and reading them beats giving up on the whole package.
        if code.is_empty() && text.is_empty() {
            return;
        }
        // Only a file that includes another can be this deep, so this is a
        // cycle rather than an unusually deep chain: the rest is unread.
        if depth >= MAX_INCLUDE_DEPTH {
            self.hidden_code = Some(INCLUDE_REASON);
            return;
        }
        for path in code {
            match parse(&path) {
                Some(ast) => self.add_file_at_depth(&ast, parent_of(&path), depth + 1),
                None => self.hidden_code = Some(INCLUDE_REASON),
            }
        }
        for path in text {
            match fs::read_to_string(&path) {
                Ok(documentation) => words_into(&mut self.names, &documentation),
                Err(_) => self.hidden_code = Some(DOC_MACRO_REASON),
            }
        }
    }
}

fn parse(path: &Path) -> Option<syn::File> {
    syn::parse_file(&fs::read_to_string(path).ok()?).ok()
}

fn parent_of(path: &Path) -> PathBuf {
    path.parent().unwrap_or(Path::new("")).to_path_buf()
}

/// Every `.rs` file belonging to the package rooted at `dir`.
///
/// Recursion stops at hidden directories, at any directory holding a
/// `Cargo.toml` (those files belong to that package, not this one), and at
/// cache directories — which is how cargo marks its `target/` directory.
fn package_sources(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !name.starts_with('.')
                    && !path.join("Cargo.toml").is_file()
                    && !path.join("CACHEDIR.TAG").is_file()
                {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// A manifest entry whose crate name the package's code never mentions.
pub struct UnusedDependency {
    /// The entry as written in `Cargo.toml`, which is the key to delete.
    pub name: String,
    /// The table it was declared in.
    pub kind: DependencyKind,
}

/// Report the dependencies of `package` that `references` never names.
pub fn find_unused(
    package: &Package,
    references: &CrateReferences,
    warnings: &mut Vec<String>,
) -> Vec<UnusedDependency> {
    if let Some(reason) = references.hidden_code {
        warnings.push(format!(
            "unused-dependency check skipped for package `{}`: {reason}",
            package.name
        ));
        return Vec::new();
    }

    // A `[features]` entry like `test = ["helper/all-features"]` is a use of
    // the dependency that no code shows: deleting the entry would break the
    // feature.
    let named_by_features = package.dependencies_named_by_features();

    let mut unused = Vec::new();
    let mut optional = Vec::new();
    let mut platform = Vec::new();

    for dependency in &package.dependencies {
        // Feature- and platform-gated entries are used by code Deadwood
        // cannot tell is compiled at all; guessing either way would be wrong
        // for somebody's feature set.
        if dependency.optional {
            optional.push(dependency.manifest_name().to_string());
            continue;
        }
        if dependency.target.is_some() {
            platform.push(dependency.manifest_name().to_string());
            continue;
        }
        let name = dependency.crate_name();
        if !references.names.contains(&name) && !named_by_features.contains(&name) {
            unused.push(UnusedDependency {
                name: dependency.manifest_name().to_string(),
                kind: dependency.dependency_kind(),
            });
        }
    }

    warn_skipped(
        warnings,
        &package.name,
        optional,
        "optional dependencies are enabled by features, which Deadwood does not evaluate yet",
    );
    warn_skipped(
        warnings,
        &package.name,
        platform,
        "`[target.'cfg(...)'.dependencies]` entries are platform-gated, which Deadwood does not evaluate yet",
    );

    unused
}

/// Surface one warning per skip reason, listing the entries it covers.
fn warn_skipped(warnings: &mut Vec<String>, package: &str, mut names: Vec<String>, reason: &str) {
    if names.is_empty() {
        return;
    }
    // The same name can appear in two tables; the reader only needs it once.
    names.sort();
    names.dedup();
    let names: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    warnings.push(format!(
        "unused-dependency check skipped for {} in package `{package}`: {reason}",
        names.join(", ")
    ));
}

/// Every identifier-shaped word in a piece of text.
fn words_into(names: &mut HashSet<String>, text: &str) {
    for word in text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
    {
        names.insert(word.to_string());
    }
}

/// The single string literal a macro was invoked with, if that is all it was
/// invoked with: `include!("generated.rs")` but not
/// `include!(concat!(env!("OUT_DIR"), "/generated.rs"))`.
fn literal_argument(tokens: &TokenStream) -> Option<String> {
    let mut trees = tokens.clone().into_iter();
    let (TokenTree::Literal(literal), None) = (trees.next()?, trees.next()) else {
        return None;
    };
    syn::parse_str::<syn::LitStr>(&literal.to_string())
        .ok()
        .map(|literal| literal.value())
}

/// The file a `#[doc = ...]` attribute takes its documentation from, when it
/// is a plain `include_str!("path")`.
fn documentation_file(value: &syn::Expr) -> Option<String> {
    let syn::Expr::Macro(mac) = value else {
        return None;
    };
    if !mac.mac.path.is_ident("include_str") {
        return None;
    }
    literal_argument(&mac.mac.tokens)
}

/// Collects every name in a file that could be naming a crate.
struct Collector<'a> {
    names: &'a mut HashSet<String>,
    hidden_code: &'a mut Option<&'static str>,
    /// Directory of the file being walked, which `include!` paths are
    /// relative to.
    dir: PathBuf,
    /// Files spliced into this one by `include!`.
    included_code: Vec<PathBuf>,
    /// Files spliced in as documentation, whose examples become doctests.
    included_text: Vec<PathBuf>,
}

impl Collector<'_> {
    fn insert(&mut self, name: String) {
        self.names.insert(name);
    }

    /// The first segment of a path: the only position a crate name can take.
    fn path_head(&mut self, path: &syn::Path) {
        if let Some(first) = path.segments.first() {
            self.insert(first.ident.to_string());
        }
    }

    /// Every segment of a path, for attribute paths, where the shape of what
    /// a macro will do with them is unknown.
    fn path_idents(&mut self, path: &syn::Path) {
        for segment in &path.segments {
            self.insert(segment.ident.to_string());
        }
    }

    /// Every identifier-shaped word in a piece of text.
    fn words_in(&mut self, text: &str) {
        words_into(self.names, text);
    }

    /// Every identifier in an unexpanded token stream, at any nesting depth.
    ///
    /// `strings` also mines string literals, which is right for attributes
    /// (where paths hide in strings) but not for macro bodies, where literals
    /// are usually data.
    fn tokens(&mut self, tokens: &TokenStream, strings: bool) {
        for tree in tokens.clone() {
            match tree {
                TokenTree::Ident(ident) => self.insert(ident.to_string()),
                TokenTree::Group(group) => self.tokens(&group.stream(), strings),
                TokenTree::Literal(literal) if strings => self.words_in(&literal.to_string()),
                _ => {}
            }
        }
    }

    /// The head of every path a `use` tree imports, including grouped ones:
    /// `use {a::b, c::d};` names both `a` and `c`.
    fn use_heads(&mut self, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(node) => self.insert(node.ident.to_string()),
            syn::UseTree::Name(node) => self.insert(node.ident.to_string()),
            syn::UseTree::Rename(node) => self.insert(node.ident.to_string()),
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.use_heads(item);
                }
            }
            // `use foo::*;` still has `foo` in its prefix, seen above.
            syn::UseTree::Glob(_) => {}
        }
    }
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.use_heads(&node.tree);
    }

    fn visit_item_extern_crate(&mut self, node: &'ast syn::ItemExternCrate) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.insert(node.ident.to_string());
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        // A bare `foo` is a local binding, a type parameter, or a unit
        // struct — never a crate. `foo::bar` and `::foo` can only be one.
        if node.leading_colon.is_some() || node.segments.len() > 1 {
            self.path_head(node);
        }
        // Generic arguments hold paths of their own.
        syn::visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        syn::visit::visit_macro(self, node);
        // `json!` may be a macro imported from a dependency, so a
        // single-segment macro path counts where a plain path would not.
        self.path_head(&node.path);
        // `include!` splices another file into this one. Its path is relative
        // to this file's directory; anything more elaborate than a literal
        // (`concat!(env!("OUT_DIR"), ..)`) names a file we cannot find.
        if node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "include")
        {
            match literal_argument(&node.tokens) {
                Some(path) => self.included_code.push(self.dir.join(path)),
                None => *self.hidden_code = Some(INCLUDE_REASON),
            }
        }
        self.tokens(&node.tokens, false);
    }

    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        match &node.meta {
            syn::Meta::Path(path) => self.path_idents(path),
            syn::Meta::List(list) => {
                self.path_idents(&list.path);
                self.tokens(&list.tokens, true);
            }
            syn::Meta::NameValue(nv) => {
                self.path_idents(&nv.path);
                match &nv.value {
                    // `#[doc = "..."]` is a doc comment: its examples are
                    // compiled as doctests, so a crate named in one is used.
                    // Other attribute strings hide real paths just as often.
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(text),
                        ..
                    }) => {
                        let text = text.value();
                        self.words_in(&text);
                    }
                    // `#![doc = include_str!("../README.md")]` is how a crate
                    // makes its README the crate documentation, examples and
                    // all — and those examples are compiled as doctests, so
                    // the file has to be read rather than given up on.
                    value => {
                        if nv.path.is_ident("doc") {
                            match documentation_file(value) {
                                Some(path) => self.included_text.push(self.dir.join(path)),
                                None => *self.hidden_code = Some(DOC_MACRO_REASON),
                            }
                        }
                        self.visit_expr(value);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::metadata::{Dependency, Package};

    /// One target's worth of source, as a package would present it.
    fn references(sources: &[&str]) -> CrateReferences {
        let mut refs = CrateReferences::default();
        for source in sources {
            refs.add_target(&[ParsedFile {
                path: PathBuf::from("/ws/src/lib.rs"),
                ast: syn::parse_file(source).ok(),
                module: Vec::new(),
            }]);
        }
        refs
    }

    fn dependency(name: &str) -> Dependency {
        Dependency {
            name: name.to_string(),
            rename: None,
            kind: None,
            optional: false,
            target: None,
        }
    }

    fn package(dependencies: Vec<Dependency>) -> Package {
        Package {
            name: "fixture".to_string(),
            manifest_path: PathBuf::from("/ws/Cargo.toml"),
            targets: Vec::new(),
            dependencies,
            features: HashMap::new(),
        }
    }

    /// Names reported unused, with the warnings the run produced.
    fn unused(package: &Package, refs: &CrateReferences) -> (Vec<String>, Vec<String>) {
        let mut warnings = Vec::new();
        let found = find_unused(package, refs, &mut warnings);
        (
            found.into_iter().map(|entry| entry.name).collect(),
            warnings,
        )
    }

    #[test]
    fn a_dependency_no_code_names_is_reported() {
        let refs = references(&["use used_crate::Thing;\nfn go(_: Thing) {}\n"]);
        let manifest = package(vec![dependency("used_crate"), dependency("dead_crate")]);
        assert_eq!(unused(&manifest, &refs).0, vec!["dead_crate"]);
    }

    /// Cargo normalizes `-` to `_` in crate names, so the manifest entry
    /// `engine-core` is spelled `engine_core` in code — and is reported by
    /// its manifest spelling.
    #[test]
    fn dashes_in_a_manifest_entry_match_underscores_in_code() {
        let refs = references(&["fn go() { engine_core::start(); }\n"]);
        let manifest = package(vec![dependency("engine-core")]);
        assert!(unused(&manifest, &refs).0.is_empty());
    }

    #[test]
    fn a_renamed_dependency_is_matched_and_reported_by_its_alias() {
        let used = Dependency {
            rename: Some("motor".to_string()),
            ..dependency("engine-core")
        };
        let dead = Dependency {
            rename: Some("gearbox".to_string()),
            ..dependency("engine-gears")
        };
        let refs = references(&["fn go() { motor::spin(); }\n"]);
        let manifest = package(vec![used, dead]);
        assert_eq!(
            unused(&manifest, &refs).0,
            vec!["gearbox"],
            "the manifest key is the alias, not the package name"
        );
    }

    /// The main false-positive trap: crates whose only appearance is somewhere
    /// macro expansion would resolve.
    #[test]
    fn names_only_macros_and_attributes_could_resolve_still_count() {
        let refs = references(&[concat!(
            "#[attr_crate(rename_all = \"camelCase\")]\n",
            "#[other(with = \"string_crate::codec\")]\n",
            "pub struct Wired;\n",
            "extern crate linked_crate;\n",
            "fn go() { println!(\"{}\", macro_body_crate::VALUE); }\n",
            "fn call() { path_macro_crate::build!(); }\n",
        )]);
        let manifest = package(vec![
            dependency("attr_crate"),
            dependency("string_crate"),
            dependency("linked_crate"),
            dependency("macro_body_crate"),
            dependency("path_macro_crate"),
        ]);
        assert!(
            unused(&manifest, &refs).0.is_empty(),
            "every unresolvable mention must keep its dependency alive"
        );
    }

    /// Doc examples are compiled as doctests, so the crates they `use` are
    /// used — often the only reason a dev-dependency exists.
    #[test]
    fn a_crate_named_only_in_a_doc_example_counts() {
        let refs = references(&[
            "/// Does a thing.\n///\n/// ```\n/// use doc_crate::helper;\n/// ```\npub fn go() {}\n",
        ]);
        let manifest = package(vec![dependency("doc_crate")]);
        assert!(unused(&manifest, &refs).0.is_empty());
    }

    /// A bare identifier is a local binding, not a crate: `let bytes = ...;`
    /// must not keep a dependency named `bytes` alive on its own.
    #[test]
    fn a_single_segment_path_is_not_a_crate_reference() {
        let refs = references(&["fn go() { let bytes = 1; let _ = bytes; }\n"]);
        let manifest = package(vec![dependency("bytes")]);
        assert_eq!(unused(&manifest, &refs).0, vec!["bytes"]);
    }

    #[test]
    fn every_target_of_the_package_satisfies_an_entry() {
        // Two targets: the lib names nothing, the build script names `cc`.
        let refs = references(&["pub fn nothing() {}\n", "fn main() { cc::Build::new(); }\n"]);
        let manifest = package(vec![dependency("cc")]);
        assert!(unused(&manifest, &refs).0.is_empty());
    }

    /// `syn` declares `syn-test-suite` only to forward a feature to it
    /// (`test = ["syn-test-suite/all-features"]`). No code names it, and
    /// deleting it would still break the feature.
    #[test]
    fn a_dependency_named_only_by_the_features_table_counts() {
        let refs = references(&["pub fn nothing() {}\n"]);
        let mut manifest = package(vec![
            dependency("forwarded"),
            dependency("enabled-by-feature"),
            dependency("dead_crate"),
        ]);
        manifest.features = HashMap::from([
            (
                "test".to_string(),
                vec!["forwarded/all-features".to_string()],
            ),
            (
                "extras".to_string(),
                vec![
                    "dep:enabled-by-feature".to_string(),
                    // A bare entry names another feature, not a dependency.
                    "test".to_string(),
                ],
            ),
        ]);
        assert_eq!(unused(&manifest, &refs).0, vec!["dead_crate"]);
    }

    #[test]
    fn optional_and_platform_gated_entries_are_skipped_with_a_warning() {
        let refs = references(&["pub fn nothing() {}\n"]);
        let manifest = package(vec![
            Dependency {
                optional: true,
                ..dependency("feature_gated")
            },
            Dependency {
                target: Some("cfg(unix)".to_string()),
                ..dependency("platform_gated")
            },
        ]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(
            unused.is_empty(),
            "gated entries are not judgeable: {unused:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("`feature_gated`")
                && w.contains("optional dependencies are enabled by features")),
            "the optional skip must be surfaced: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("`platform_gated`") && w.contains("platform-gated")),
            "the platform skip must be surfaced: {warnings:?}"
        );
    }

    /// The usual `include!` target is generated into `OUT_DIR`, whose path is
    /// only known at build time: that code can hold the only reference to a
    /// dependency and we will never see it.
    #[test]
    fn an_include_we_cannot_resolve_skips_the_whole_package() {
        let refs = references(&["include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n"]);
        let manifest = package(vec![dependency("dead_crate")]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(unused.is_empty(), "unseen code may hold the only reference");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unused-dependency check skipped") && w.contains("include!")),
            "the skip must be surfaced: {warnings:?}"
        );
    }

    /// A readable `include!` is followed instead: the names in the included
    /// file are the including file's names.
    #[test]
    fn a_readable_include_is_followed() {
        let mut refs = CrateReferences::default();
        // Nothing reads `probe.rs` itself — only the directory it names
        // matters, and `deps.rs` next to it certainly exists.
        refs.add_target(&[ParsedFile {
            path: Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs"),
            ast: syn::parse_file("include!(\"deps.rs\");\n").ok(),
            module: Vec::new(),
        }]);
        let manifest = package(vec![dependency("proc-macro2")]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(
            unused.is_empty(),
            "the included file uses `proc_macro2`: {unused:?}"
        );
        assert!(
            warnings.is_empty(),
            "an include we can follow is not a skip: {warnings:?}"
        );
    }

    /// `#![doc = include_str!("../README.md")]` makes a file the crate
    /// documentation, and its examples become doctests that can use any
    /// dependency — so the file is read.
    #[test]
    fn documentation_included_from_a_file_is_read() {
        let mut refs = CrateReferences::default();
        refs.add_target(&[ParsedFile {
            path: Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs"),
            ast: syn::parse_file("#![doc = include_str!(\"deps.rs\")]\n").ok(),
            module: Vec::new(),
        }]);
        let manifest = package(vec![dependency("proc-macro2")]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(
            unused.is_empty(),
            "the file names `proc_macro2`: {unused:?}"
        );
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    /// Documentation from anywhere else — a `concat!`, a macro of the crate's
    /// own — is a file we cannot open, and its doctests stay invisible.
    #[test]
    fn documentation_we_cannot_read_skips_the_whole_package() {
        let refs = references(&["#![doc = concat!(\"a\", \"b\")]\npub fn go() {}\n"]);
        let manifest = package(vec![dependency("dead_crate")]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(unused.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unused-dependency check skipped")),
            "the skip must be surfaced: {warnings:?}"
        );
    }

    #[test]
    fn an_unparsable_file_skips_the_whole_package() {
        let refs = references(&["fn oops( {\n"]);
        let manifest = package(vec![dependency("dead_crate")]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(unused.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unused-dependency check skipped")),
            "the skip must be surfaced: {warnings:?}"
        );
    }
}
