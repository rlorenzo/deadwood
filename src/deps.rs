//! Dependency checks: entries nothing names, and entries in the wrong table.
//!
//! A dependency declared in a package's `Cargo.toml` that the package's code
//! never refers to is dead weight: it slows builds, widens the supply-chain
//! surface, and misleads readers about what the crate actually needs. Cargo
//! never complains, because it has no reason to look.
//!
//! Two questions are asked of every entry, and they are deliberately separate
//! functions with separate finding kinds:
//!
//! - [`find_unused`] — *does anything in this package name the crate at all?*
//! - [`find_misplaced`] — *is the entry in a table the code that names it can
//!   see?* A `[dependencies]` entry only the tests use belongs in
//!   `[dev-dependencies]`, where it stays out of every consumer's build.
//!
//! Folding the second into the first was tried on paper and rejected; the
//! reasoning is under "Dependency kinds" below, and it is the reason
//! [`find_unused`] still judges an entry against every target of its package.
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
//! # Gated entries
//!
//! Optional and `[target.'cfg(...)'.dependencies]` entries used to be skipped
//! outright: both are reached through code behind a `cfg`, and Deadwood did
//! not evaluate one. It does now ([`crate::cfg`]), and the answer under the
//! default matrix turns out to be simple — every feature combination and every
//! target is analyzed, so the code that uses such an entry *is* read, and a
//! reference to it is found wherever one exists. They are therefore judged
//! like any other entry.
//!
//! A configured matrix is the case that still cannot be judged. If
//! `deadwood.toml` narrows features so that nothing can turn an optional
//! dependency on, or narrows targets so that a `[target.'cfg(...)']` table
//! never applies, then the code using that entry was never read and "never
//! referenced" would be a statement about a build that was not analyzed. Those
//! entries are skipped with a warning naming the matrix as the reason.
//!
//! One consequence is worth stating, because it looks like a special case and
//! is not: Cargo synthesizes a `foo = ["dep:foo"]` feature for every optional
//! dependency, and [`crate::metadata::Package::dependencies_named_by_features`]
//! deliberately does not count it. It is the entry restated, not a second
//! place naming it, and counting it would leave every optional dependency
//! permanently unjudgeable.
//!
//! # What is skipped, and why
//!
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
//! Each skip is surfaced as a warning rather than guessed at, once per check,
//! since silencing one detector says nothing about the other.
//!
//! # Dependency kinds
//!
//! [`find_unused`] judges a manifest entry against references from *every*
//! target of its package, not only the targets that can legitimately see it.
//! Narrowing that per kind — `[dev-dependencies]` against tests only,
//! `[dependencies]` against lib and bins only — would turn "declared in the
//! wrong table" into an unused-dependency finding, and a user staring at
//! `cc::Build::new()` in their `build.rs` would rightly call that a false
//! positive. Whether an entry sits in the right table is a different question
//! with a different noise profile, so it is a different check with a finding
//! kind of its own. The reported kind still comes from the entry, so the
//! unused message names the table to edit.
//!
//! ## Where a mention counts: [`Contexts`]
//!
//! [`find_misplaced`] needs what the unused check does not: *which* code names
//! a crate. Every mention is therefore attributed to one of four places —
//! runtime targets (lib, bins, proc-macro), dev targets (test, example,
//! bench), the build script, or nowhere in particular — and a name accumulates
//! the set of places it was seen in. An entry is misplaced only when *every*
//! mention of it lands outside what its table serves.
//!
//! Two attributions are load bearing enough to state on their own, because
//! getting either wrong is what would sink the check.
//!
//! **`#[cfg(test)]` code is dev code, wherever it sits.** The unit tests of a
//! library live in the library target and still link the `[dev-dependencies]`,
//! so a naive per-target split calls every dev-dependency used by a
//! `#[cfg(test)] mod tests` misplaced — in essentially every crate that has
//! one. [`crate::cfg::Gates::test_only`] answers "does this gate confine the
//! item to a test build?" against the maximal matrix, and anything it answers
//! yes for moves its whole subtree into the dev context. Judging it against
//! the maximal matrix rather than the configured one is deliberate: where an
//! item can be compiled is a property of the code, not of what the user asked
//! to analyze.
//!
//! **A doc comment attributes to nowhere.** Doc examples are compiled as
//! doctests, and a doctest links the normal *and* the dev dependencies — so a
//! dependency named only in a doc example is correctly declared under either
//! table, and a mention there proves nothing about placement in either
//! direction. The mining is word-level besides ("`itoa`" in prose is
//! indistinguishable from `itoa::Buffer`), which is fine for keeping an entry
//! alive and far too weak to move one. Doc words therefore land in the opaque
//! context, which every table serves, so they can only ever silence a finding.
//!
//! The same reasoning covers the other channels that name a crate without
//! placing it, and they land in the same context: identifiers inside macro
//! input (a `macro_rules!` body expands wherever it is invoked, which may be a
//! different target entirely — serde_json's only mention of `itoa` outside
//! plain code is exactly that), identifiers and strings in attribute
//! arguments, and every name in a `.rs` file that no `mod` declaration
//! reaches. That last one is worth spelling out: those files are read at all
//! *because* a macro we cannot expand declares them (`automod::dir!`), and
//! that macro is the only thing that knows which target compiles them.
//!
//! ## Which claims are made
//!
//! "No target of the right kind names it" is a weaker statement than "it is in
//! the wrong table", and only two directions clear the gap:
//!
//! - A `[dependencies]` entry every mention of which is dev code belongs in
//!   `[dev-dependencies]`. Nothing is lost by moving it and every consumer of
//!   the crate stops building it.
//! - A `[build-dependencies]` entry the build script never names is in a table
//!   nothing reads, since the build script is the only thing compiled against
//!   that table. The code that *does* name it says which table it should have
//!   been in.
//!
//! Three directions are deliberately never reported. An entry nothing names is
//! [`find_unused`]'s answer, not this one's. An entry mentioned only opaquely
//! is not placed, in any direction. And a `[dev-dependencies]` entry the
//! library appears to name is left alone: such a manifest does not compile at
//! all, so the likelier explanation is that we attributed the mention wrongly.
//! The largest source of that mis-attribution is closed — an out-of-line
//! `#[cfg(test)] mod tests;` now arrives here as the test code it is
//! ([`ParsedFile::test_only`]) — but the direction stays unreported until it
//! has evidence of its own rather than the absence of a known gap.
//!
//! # Entries that are load bearing without being named
//!
//! A dependency declared only to turn on a feature of a *transitive*
//! dependency (the `getrandom = { features = ["js"] }` idiom), to select a
//! vendored native library (`openssl = { features = ["vendored"] }`), or to
//! force feature unification across a workspace is referenced by no code and
//! by no `[features]` entry of its own package. There is no syntactic signal
//! that separates it from a stale entry — the source genuinely never mentions
//! the crate — so the answer is user intent: the `[dependencies]` allowlist in
//! `deadwood.toml` (see [`crate::config`]) names such entries by their
//! manifest key, workspace-wide or per package, and they are never judged.
//! Allowlisting an entry that *is* referenced is not an error; the list says
//! "do not judge this", not "assert this is unused".
//!
//! # Known limitations
//!
//! - A dependency reachable only through a glob import of another crate's
//!   prelude (a derive macro re-exported by a facade crate) is invisible.
//! - Only the source tree in front of us counts. A crate unpacked from a
//!   published `.crate` archive usually has its `tests/` and `benches/`
//!   stripped, so the dev-dependencies those used are correctly reported as
//!   unreferenced *by that tree* — and would not be, in the repository it was
//!   published from. The placement check is unaffected in the direction that
//!   matters: fewer dev targets means less evidence, and less evidence never
//!   turns into a finding.
//! - Placement is only as good as the attribution. One mention through a
//!   macro, an attribute or a doc comment is enough to make an entry
//!   unplaceable, which is most of the recall the check gives up — across the
//!   34 crates in the local registry it is the reason every entry declared in
//!   the wrong table would have to be found by hand.
//! - A file two `mod` declarations reach, one confining it to a test build and
//!   one not, is attributed to the ungated one — all of it, including the
//!   parts only the test-confined declaration compiles. There is one file and
//!   one answer, and this is the direction that misses findings instead of
//!   inventing them ([`crate::modtree::resolve`]).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;

use crate::cfg::{Gates, TargetVerdict};
use crate::config::DependencyAllowList;
use crate::metadata::{DependencyKind, Package, Target};
use crate::modtree::ParsedFile;

/// Why a package's reference set is incomplete, phrased for a warning.
const INCLUDE_REASON: &str = "code is pulled in with `include!` from a file that could not be read";
const DOC_MACRO_REASON: &str =
    "documentation is generated by a macro, so the doctests in it are not analyzed";
const DOC_FILE_REASON: &str = "documentation is included from a file that could not be read, so the doctests in it are not \
     analyzed";
const UNPARSABLE_REASON: &str = "a source file could not be read or parsed";

/// How deep a chain of `include!`d files is followed. Real code never nests.
const MAX_INCLUDE_DEPTH: usize = 8;

/// Whether a target is test code in its entirety.
///
/// A test, example or bench target is built by `cargo test`/`cargo bench` and
/// run by nothing that consumes the package, so everything in one is test code
/// — not only the functions carrying `#[test]`. This check answers both the
/// question "which table does a mention of this crate name place an entry in"
/// ([`Contexts::of_target`]) and "is an entry point written here reached by a
/// build with no tests in it" ([`crate::resolve`]); they are the same question
/// about the same targets, and two copies of the list could disagree.
pub(crate) fn is_dev_target(target: &Target) -> bool {
    target
        .kind
        .iter()
        .any(|kind| kind == "test" || kind == "example" || kind == "bench")
}

/// Which code of a package a mention of a crate name was found in.
///
/// A set rather than a single value, because a crate name is usually mentioned
/// in several places and the placement question is about all of them at once:
/// an entry sits in the wrong table only when *every* mention of it lands
/// somewhere that table does not serve.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Contexts(u8);

impl Contexts {
    /// Library, binary and proc-macro targets: the code that ships to
    /// consumers, and the only code a `[dependencies]` entry exists for.
    const RUNTIME: Contexts = Contexts(1 << 0);
    /// Test, example and bench targets, plus `#[cfg(test)]` code inside any
    /// other target. All of it links `[dev-dependencies]`.
    const DEV: Contexts = Contexts(1 << 1);
    /// The build script, the only target a `[build-dependencies]` entry is
    /// compiled for.
    const BUILD_SCRIPT: Contexts = Contexts(1 << 2);
    /// A mention no target can be held responsible for. See the module docs:
    /// this is what keeps doc comments, macro input and files no `mod`
    /// declaration names from proving anything about placement.
    const OPAQUE: Contexts = Contexts(1 << 3);

    /// Where the files of `target` are attributed.
    fn of_target(target: &Target) -> Contexts {
        if target.kind.iter().any(|kind| kind == "custom-build") {
            Contexts::BUILD_SCRIPT
        } else if is_dev_target(target) {
            Contexts::DEV
        } else {
            Contexts::RUNTIME
        }
    }

    fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn contains(self, other: Contexts) -> bool {
        self.0 & other.0 == other.0
    }

    fn insert(&mut self, other: Contexts) {
        self.0 |= other.0;
    }
}

/// Where one file's mentions are attributed, and what decides it.
#[derive(Clone, Copy)]
struct Origin<'a> {
    context: Contexts,
    /// `None` for a file no target claims, where nothing is attributable
    /// anyway and there is no package to evaluate `#[cfg(test)]` against.
    gates: Option<&'a Gates<'a>>,
}

/// Every crate name a package's code could be referring to, and where.
#[derive(Default)]
pub struct CrateReferences {
    /// Name to the set of places it was mentioned. The keys alone answer the
    /// unused-dependency question; the values answer the placement one.
    names: HashMap<String, Contexts>,
    /// Set when the package pulls in code Deadwood never sees, which would
    /// make any "never referenced" verdict a guess.
    hidden_code: Option<&'static str>,
}

impl CrateReferences {
    /// Add every crate name the files of one target refer to, attributed to
    /// the code that target is.
    pub fn add_target(&mut self, files: &[ParsedFile], target: &Target, gates: &Gates<'_>) {
        let context = Contexts::of_target(target);
        for file in files {
            // A file that only `#[cfg(test)] mod tests;` reaches is unit-test
            // code exactly as the inline form is, but the gate saying so is
            // written in the parent — nothing in this file could tell us, so
            // module resolution carries the answer here.
            let context = if context == Contexts::RUNTIME && file.test_only {
                Contexts::DEV
            } else {
                context
            };
            let origin = Origin {
                context,
                gates: Some(gates),
            };
            match &file.ast {
                Some(ast) => self.add_file(ast, parent_of(&file.path), origin),
                None => self.hidden_code = Some(UNPARSABLE_REASON),
            }
        }
    }

    /// Add every crate name mentioned by a `.rs` file under `dir` that the
    /// module tree did not already reach.
    ///
    /// `already_read` holds the files [`Self::add_target`] has covered, taken
    /// from module resolution; the rest are read and parsed here.
    ///
    /// Everything found this way is [`Contexts::OPAQUE`]. The reason these
    /// files are read at all is that a macro we cannot expand declares them
    /// (`automod::dir!`), and that macro is the only thing that knows which
    /// target compiles them — so a mention here says a crate is used, and
    /// nothing about where.
    pub fn add_unreached_sources(&mut self, dir: &Path, already_read: &HashSet<PathBuf>) {
        let origin = Origin {
            context: Contexts::OPAQUE,
            gates: None,
        };
        for path in package_sources(dir) {
            if already_read.contains(&path) {
                continue;
            }
            match parse(&path) {
                Some(ast) => self.add_file(&ast, parent_of(&path), origin),
                // Unlike module resolution, this walk is speculative: it finds
                // files nothing declares. One we cannot read may still be
                // compiled, and may hold the only reference to a dependency.
                None => self.hidden_code = Some(UNPARSABLE_REASON),
            }
        }
    }

    fn add_file(&mut self, ast: &syn::File, dir: PathBuf, origin: Origin<'_>) {
        self.add_file_at_depth(ast, dir, origin, 0);
    }

    fn add_file_at_depth(
        &mut self,
        ast: &syn::File,
        dir: PathBuf,
        origin: Origin<'_>,
        depth: usize,
    ) {
        // An inner `#![cfg(test)]` makes the whole file unit-test code, wherever
        // its target would otherwise put it.
        let mut origin = origin;
        origin.context = test_shifted(origin, &ast.attrs);

        let mut collector = Collector {
            names: &mut self.names,
            hidden_code: &mut self.hidden_code,
            dir,
            origin,
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
        for (path, context) in code {
            // An included file is spliced into the item that includes it, so
            // it is that item's code: same target, and same `#[cfg(test)]`
            // context, which is why the site's context travels with the path
            // rather than the file's being reused here.
            let origin = Origin { context, ..origin };
            match parse(&path) {
                Some(ast) => self.add_file_at_depth(&ast, parent_of(&path), origin, depth + 1),
                None => self.hidden_code = Some(INCLUDE_REASON),
            }
        }
        for path in text {
            match fs::read_to_string(&path) {
                // Documentation, wherever it came from: opaque like every
                // other doc comment.
                Ok(documentation) => {
                    words_into(&mut self.names, &documentation, Contexts::OPAQUE);
                }
                Err(_) => self.hidden_code = Some(DOC_FILE_REASON),
            }
        }
    }
}

/// `origin`'s context, moved to [`Contexts::DEV`] when `attrs` confine the
/// item they sit on to a test build.
///
/// Only runtime code moves. A test target is already dev code, an opaque
/// mention must not become attributable, and a build script's `#[cfg(test)]`
/// module is not compiled by `cargo test` at all — it is still build-script
/// code, and treating it as dev code would be inventing a claim.
fn test_shifted(origin: Origin<'_>, attrs: &[syn::Attribute]) -> Contexts {
    let test_only = origin.gates.is_some_and(|gates| gates.test_only(attrs));
    if origin.context == Contexts::RUNTIME && test_only {
        Contexts::DEV
    } else {
        origin.context
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

/// Report the dependencies of `package` that `references` never names, other
/// than the manifest keys `allowed` exempts from the check entirely.
pub fn find_unused(
    package: &Package,
    references: &CrateReferences,
    allowed: &DependencyAllowList,
    gates: &Gates<'_>,
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
    let mut never_built = Vec::new();

    for dependency in &package.dependencies {
        // Before anything else, including the skip warnings: an allowlisted
        // entry is one the user has already ruled on, and reporting that we
        // declined to judge it would be noise about noise.
        if allowed.allows(&package.name, dependency.manifest_name()) {
            continue;
        }
        // Feature- and platform-gated entries are reached through code behind
        // a `cfg`. The default matrix analyzes that code, so they are judged
        // like anything else; a matrix that narrows it away leaves the entry
        // unjudgeable, because the only code that could name it was not read.
        if dependency.optional && !gates.optional_dependency_possible(dependency.manifest_name()) {
            optional.push(dependency.manifest_name().to_string());
            continue;
        }
        if let Some(target) = &dependency.target {
            match gates.target_expression(target) {
                TargetVerdict::Possible => {}
                TargetVerdict::RuledOutByMatrix => {
                    platform.push(dependency.manifest_name().to_string());
                    continue;
                }
                TargetVerdict::NeverBuilt => {
                    never_built.push(dependency.manifest_name().to_string());
                    continue;
                }
            }
        }
        let name = dependency.crate_name();
        if !references.names.contains_key(&name) && !named_by_features.contains(&name) {
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
        "no feature the configured `[cfg] features` matrix enables can turn these optional entries \
         on, so the code that would name them was never analyzed",
    );
    warn_skipped(
        warnings,
        &package.name,
        platform,
        "the configured `[cfg] target-os` matrix rules out the \
         `[target.'cfg(...)'.dependencies]` table these came from, so the code that would name \
         them was never analyzed",
    );
    warn_skipped(
        warnings,
        &package.name,
        never_built,
        "the `[target.'cfg(...)'.dependencies]` table these came from holds on no target at all, \
         so they are declared to constrain version resolution rather than to be compiled",
    );

    unused
}

/// A manifest entry every mention of which lands outside the code its table
/// serves.
pub struct MisplacedDependency {
    /// The entry as written in `Cargo.toml`, which is the key to move.
    pub name: String,
    /// The table it is declared in.
    pub declared: DependencyKind,
    /// The table the references say it belongs in.
    pub belongs_in: DependencyKind,
}

/// Report the dependencies of `package` that are declared in a table no code
/// referencing them can see.
///
/// The skips are [`find_unused`]'s, for the same reasons: an entry whose
/// references may be missing cannot be placed any more than it can be
/// declared dead. Only the package-scope one is warned about here — repeating
/// the per-entry lists would double the noisiest warnings a run produces
/// without adding a fact, since both checks skip the same entries for the same
/// reason and [`find_unused`] has already named them.
pub fn find_misplaced(
    package: &Package,
    references: &CrateReferences,
    allowed: &DependencyAllowList,
    gates: &Gates<'_>,
    warnings: &mut Vec<String>,
) -> Vec<MisplacedDependency> {
    if let Some(reason) = references.hidden_code {
        warnings.push(format!(
            "misplaced-dependency check skipped for package `{}`: {reason}",
            package.name
        ));
        return Vec::new();
    }

    let named_by_features = package.dependencies_named_by_features();

    let mut misplaced = Vec::new();
    for dependency in &package.dependencies {
        if allowed.allows(&package.name, dependency.manifest_name()) {
            continue;
        }
        if dependency.optional && !gates.optional_dependency_possible(dependency.manifest_name()) {
            continue;
        }
        if let Some(target) = &dependency.target
            && gates.target_expression(target) != TargetVerdict::Possible
        {
            continue;
        }
        // An entry the `[features]` table names is load bearing for a reason
        // that has no code and therefore no target — and `[features]` cannot
        // refer to a dev-dependency at all, so moving it would break the
        // feature that names it.
        if named_by_features.contains(&dependency.crate_name()) {
            continue;
        }
        let found = references
            .names
            .get(&dependency.crate_name())
            .copied()
            .unwrap_or_default();
        if let Some(belongs_in) = misplacement(dependency.dependency_kind(), found) {
            misplaced.push(MisplacedDependency {
                name: dependency.manifest_name().to_string(),
                declared: dependency.dependency_kind(),
                belongs_in,
            });
        }
    }
    misplaced
}

/// The table `found` says an entry declared as `kind` belongs in, or `None`
/// when the evidence does not support moving it.
///
/// Every `None` here is deliberate; see the module docs for the reasoning
/// behind each.
fn misplacement(kind: DependencyKind, found: Contexts) -> Option<DependencyKind> {
    // Nothing names it: that is the unused-dependency check's question. And a
    // mention we could not attribute to a target proves a use without proving
    // where, which is exactly the evidence a placement claim needs.
    if found.is_empty() || found.contains(Contexts::OPAQUE) {
        return None;
    }
    match kind {
        // Test, example and bench code links `[dev-dependencies]`, so an entry
        // only they name costs every consumer of the crate a build for nothing.
        DependencyKind::Normal if found == Contexts::DEV => Some(DependencyKind::Development),
        // The build script is the only thing that compiles against a
        // `[build-dependencies]` entry, so one it never names is in the wrong
        // table — and the code that does name it says which.
        DependencyKind::Build if !found.contains(Contexts::BUILD_SCRIPT) => {
            Some(if found.contains(Contexts::RUNTIME) {
                DependencyKind::Normal
            } else {
                DependencyKind::Development
            })
        }
        _ => None,
    }
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
fn words_into(names: &mut HashMap<String, Contexts>, text: &str, context: Contexts) {
    for word in text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
    {
        names.entry(word.to_string()).or_default().insert(context);
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
struct Collector<'a, 'g> {
    names: &'a mut HashMap<String, Contexts>,
    hidden_code: &'a mut Option<&'static str>,
    /// Directory of the file being walked, which `include!` paths are
    /// relative to.
    dir: PathBuf,
    /// Where the mentions found here are attributed. Its context follows
    /// `#[cfg(test)]` down the item tree.
    origin: Origin<'g>,
    /// Files spliced into this one by `include!`, each with the context of
    /// the item the `include!` was written in — the included code is that
    /// item's code, gate and all.
    included_code: Vec<(PathBuf, Contexts)>,
    /// Files spliced in as documentation, whose examples become doctests.
    included_text: Vec<PathBuf>,
}

impl Collector<'_, '_> {
    /// A mention we can attribute to the code it was written in: a path head,
    /// a `use`, an `extern crate`, a macro's own name.
    fn insert(&mut self, name: String) {
        let context = self.origin.context;
        self.names.entry(name).or_default().insert(context);
    }

    /// A mention that names a crate without saying which code uses it.
    fn insert_opaque(&mut self, name: String) {
        self.names.entry(name).or_default().insert(Contexts::OPAQUE);
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
            self.insert_opaque(segment.ident.to_string());
        }
    }

    /// Every identifier-shaped word in a piece of text.
    fn words_in(&mut self, text: &str) {
        words_into(self.names, text, Contexts::OPAQUE);
    }

    /// Walk `body` with the context `attrs` puts it in, then restore.
    fn within(&mut self, attrs: &[syn::Attribute], body: impl FnOnce(&mut Self)) {
        let outer = self.origin.context;
        self.origin.context = test_shifted(self.origin, attrs);
        body(self);
        self.origin.context = outer;
    }

    /// Every identifier in an unexpanded token stream, at any nesting depth.
    ///
    /// `strings` also mines string literals, which is right for attributes
    /// (where paths hide in strings) but not for macro bodies, where literals
    /// are usually data.
    fn tokens(&mut self, tokens: &TokenStream, strings: bool) {
        for tree in tokens.clone() {
            match tree {
                TokenTree::Ident(ident) => self.insert_opaque(ident.to_string()),
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

impl<'ast> Visit<'ast> for Collector<'_, '_> {
    // `#[cfg(test)]` moves everything under it into the package's test code,
    // wherever the item sits. Four entry points cover every place an item can
    // be written: the module tree, `impl` blocks, `trait` bodies and
    // `extern` blocks. Items inside function bodies arrive through
    // `visit_item` as well, since `syn` walks a `Stmt::Item` into it.
    fn visit_item(&mut self, node: &'ast syn::Item) {
        self.within(crate::cfg::attrs_of(node), |this| {
            syn::visit::visit_item(this, node);
        });
    }

    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        self.within(crate::cfg::impl_item_attrs(node), |this| {
            syn::visit::visit_impl_item(this, node);
        });
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        self.within(crate::cfg::trait_item_attrs(node), |this| {
            syn::visit::visit_trait_item(this, node);
        });
    }

    fn visit_foreign_item(&mut self, node: &'ast syn::ForeignItem) {
        self.within(crate::cfg::foreign_item_attrs(node), |this| {
            syn::visit::visit_foreign_item(this, node);
        });
    }

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
                Some(path) => {
                    let at = self.dir.join(path);
                    self.included_code.push((at, self.origin.context));
                }
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
        let sources: Vec<(&str, &str)> = sources.iter().map(|source| ("lib", *source)).collect();
        references_from(&sources)
    }

    /// Several targets' worth of source, each with the target kind
    /// `cargo metadata` would report for it.
    fn references_from(sources: &[(&str, &str)]) -> CrateReferences {
        let mut refs = CrateReferences::default();
        for (kind, source) in sources {
            add_one(
                &mut refs,
                kind,
                PathBuf::from("/ws/src/lib.rs"),
                syn::parse_file(source).ok(),
            );
        }
        refs
    }

    /// One file, added as the sole file of a target of the given kind.
    fn add_one(refs: &mut CrateReferences, kind: &str, path: PathBuf, ast: Option<syn::File>) {
        add_one_reached(refs, kind, path, ast, false);
    }

    /// The same, for a file only test-confined `mod` declarations reach.
    fn add_one_reached(
        refs: &mut CrateReferences,
        kind: &str,
        path: PathBuf,
        ast: Option<syn::File>,
        test_only: bool,
    ) {
        let manifest = crate::cfg::tests_support::bare_package();
        let matrix = crate::cfg::Matrix::default();
        let gates = Gates::new(&matrix, &manifest);
        refs.add_target(
            &[ParsedFile {
                path,
                ast,
                module: Vec::new(),
                test_only,
                test_only_mods: Vec::new(),
            }],
            &target(kind),
            &gates,
        );
    }

    fn target(kind: &str) -> Target {
        Target {
            name: "fixture".to_string(),
            kind: vec![kind.to_string()],
            src_path: PathBuf::from("/ws/src/lib.rs"),
        }
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

    /// Names reported unused, with the warnings the run produced, under the
    /// default `cfg` matrix.
    fn unused(package: &Package, refs: &CrateReferences) -> (Vec<String>, Vec<String>) {
        unused_with(package, refs, &crate::cfg::Matrix::default())
    }

    fn unused_with(
        package: &Package,
        refs: &CrateReferences,
        matrix: &crate::cfg::Matrix,
    ) -> (Vec<String>, Vec<String>) {
        let mut warnings = Vec::new();
        let found = find_unused(
            package,
            refs,
            &DependencyAllowList::default(),
            &Gates::new(matrix, package),
            &mut warnings,
        );
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

    /// Both used to be skipped outright. Under the default matrix every
    /// feature and every target is analyzed, so the code that would name them
    /// *was* read — and an entry nothing named is an entry nothing named.
    #[test]
    fn gated_entries_are_judged_under_the_default_matrix() {
        let gated = || {
            package(vec![
                Dependency {
                    optional: true,
                    ..dependency("feature_gated")
                },
                Dependency {
                    target: Some("cfg(unix)".to_string()),
                    ..dependency("platform_gated")
                },
            ])
        };

        let refs = references(&["pub fn nothing() {}\n"]);
        let (mut reported, warnings) = unused(&gated(), &refs);
        reported.sort();
        assert_eq!(reported, vec!["feature_gated", "platform_gated"]);
        assert!(warnings.is_empty(), "nothing was skipped: {warnings:?}");

        // And a mention behind the very `cfg` that gates them still counts,
        // because the default matrix compiles it.
        let refs = references(&[concat!(
            "#[cfg(feature = \"feature_gated\")]\nfn a() { feature_gated::go(); }\n",
            "#[cfg(unix)]\nfn b() { platform_gated::go(); }\n",
        )]);
        assert!(unused(&gated(), &refs).0.is_empty());
    }

    /// A matrix that narrows the gate away is the case that stays unjudgeable:
    /// the only code that could name the entry was never read, so "never
    /// referenced" would describe a build nobody analyzed.
    #[test]
    fn a_narrowed_matrix_skips_the_entries_it_rules_out_with_a_warning() {
        let mut manifest = package(vec![
            Dependency {
                optional: true,
                ..dependency("feature_gated")
            },
            Dependency {
                target: Some("cfg(windows)".to_string()),
                ..dependency("platform_gated")
            },
        ]);
        manifest.features = HashMap::from([(
            "feature_gated".to_string(),
            vec!["dep:feature_gated".to_string()],
        )]);

        let refs = references(&["pub fn nothing() {}\n"]);
        let matrix =
            crate::cfg::Matrix::new(Some(Vec::new()), Some(vec!["linux".to_string()]), None);
        let (reported, warnings) = unused_with(&manifest, &refs, &matrix);
        assert!(
            reported.is_empty(),
            "an entry the matrix gates away is not judgeable: {reported:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("`feature_gated`") && w.contains("[cfg] features")),
            "the optional skip must name the matrix as the reason: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("`platform_gated`") && w.contains("[cfg] target-os")),
            "the platform skip must name the matrix as the reason: {warnings:?}"
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
        add_one(
            &mut refs,
            "lib",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs"),
            syn::parse_file("include!(\"deps.rs\");\n").ok(),
        );
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
        add_one(
            &mut refs,
            "lib",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs"),
            syn::parse_file("#![doc = include_str!(\"deps.rs\")]\n").ok(),
        );
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
                .any(|w| w.contains("documentation is generated by a macro")),
            "the skip must name the reason it happened: {warnings:?}"
        );
    }

    /// A named documentation file that cannot be opened is a different
    /// problem from documentation a macro builds, and says so: the reader's
    /// fix is a path, not a macro.
    #[test]
    fn documentation_from_a_missing_file_names_that_as_the_reason() {
        let mut refs = CrateReferences::default();
        add_one(
            &mut refs,
            "lib",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs"),
            syn::parse_file("#![doc = include_str!(\"no_such_file.md\")]\n").ok(),
        );
        let manifest = package(vec![dependency("dead_crate")]);
        let (unused, warnings) = unused(&manifest, &refs);
        assert!(unused.is_empty(), "unread docs may hold the only reference");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("documentation is included from a file that could not be read")),
            "the skip must name the reason it happened: {warnings:?}"
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

    // -- placement ---------------------------------------------------------

    fn dev_dependency(name: &str) -> Dependency {
        Dependency {
            kind: Some("dev".to_string()),
            ..dependency(name)
        }
    }

    fn build_dependency(name: &str) -> Dependency {
        Dependency {
            kind: Some("build".to_string()),
            ..dependency(name)
        }
    }

    /// Entries reported misplaced, as `(entry, table it belongs in)`, with the
    /// warnings the run produced.
    fn misplaced(
        package: &Package,
        refs: &CrateReferences,
    ) -> (Vec<(String, DependencyKind)>, Vec<String>) {
        misplaced_allowing(package, refs, &DependencyAllowList::default())
    }

    fn misplaced_allowing(
        package: &Package,
        refs: &CrateReferences,
        allowed: &DependencyAllowList,
    ) -> (Vec<(String, DependencyKind)>, Vec<String>) {
        let matrix = crate::cfg::Matrix::default();
        let mut warnings = Vec::new();
        let found = find_misplaced(
            package,
            refs,
            allowed,
            &Gates::new(&matrix, package),
            &mut warnings,
        );
        (
            found
                .into_iter()
                .map(|entry| (entry.name, entry.belongs_in))
                .collect(),
            warnings,
        )
    }

    /// The finding the check exists for: a normal entry no shipping code
    /// names, only a test target — which links `[dev-dependencies]` anyway.
    #[test]
    fn a_normal_entry_only_a_test_target_names_belongs_in_dev_dependencies() {
        let refs = references_from(&[
            ("lib", "pub fn go() {}\n"),
            ("test", "fn t() { test_helper::assert_ok(); }\n"),
        ]);
        let manifest = package(vec![dependency("test_helper")]);
        assert_eq!(
            misplaced(&manifest, &refs).0,
            vec![("test_helper".to_string(), DependencyKind::Development)]
        );
    }

    /// The out-of-line half of the `#[cfg(test)] mod` case. The file holds no
    /// gate of its own — the one that confines it is written in its parent —
    /// so the answer has to arrive with the file, from module resolution.
    #[test]
    fn a_file_only_a_test_confined_module_reaches_is_test_code() {
        let mut refs = CrateReferences::default();
        add_one_reached(
            &mut refs,
            "lib",
            PathBuf::from("/ws/src/lib.rs"),
            syn::parse_file("pub fn go() {}\n").ok(),
            false,
        );
        add_one_reached(
            &mut refs,
            "lib",
            PathBuf::from("/ws/src/tests.rs"),
            syn::parse_file("fn t() { test_helper::assert_ok(); }\n").ok(),
            true,
        );
        let manifest = package(vec![dependency("test_helper")]);
        assert_eq!(
            misplaced(&manifest, &refs).0,
            vec![("test_helper".to_string(), DependencyKind::Development)],
            "the lib target holds the file, but only its tests reach it"
        );
    }

    /// The same file without the flag: a lib file naming a `[dependencies]`
    /// entry is the entry doing its job, and reporting it would be the false
    /// positive the flag has to be careful not to invent.
    #[test]
    fn the_same_file_reached_by_ordinary_code_is_not() {
        let mut refs = CrateReferences::default();
        add_one_reached(
            &mut refs,
            "lib",
            PathBuf::from("/ws/src/view.rs"),
            syn::parse_file("fn t() { test_helper::assert_ok(); }\n").ok(),
            false,
        );
        let manifest = package(vec![dependency("test_helper")]);
        assert!(
            misplaced(&manifest, &refs).0.is_empty(),
            "the library itself names it: {:?}",
            misplaced(&manifest, &refs).0
        );
    }

    /// Examples and benches link the dev-dependencies exactly as tests do.
    #[test]
    fn example_and_bench_targets_count_as_test_code() {
        for kind in ["example", "bench"] {
            let refs = references_from(&[
                ("lib", "pub fn go() {}\n"),
                (kind, "fn m() { only::go(); }\n"),
            ]);
            let manifest = package(vec![dependency("only")]);
            assert_eq!(
                misplaced(&manifest, &refs).0,
                vec![("only".to_string(), DependencyKind::Development)],
                "a `{kind}` target is dev code"
            );
        }
    }

    /// The single largest false positive a naive per-target split makes:
    /// `#[cfg(test)]` code lives in the lib target and still links the
    /// dev-dependencies, so an entry only it names is exactly where it belongs.
    #[test]
    fn a_dev_dependency_used_by_a_cfg_test_module_in_the_lib_is_correctly_placed() {
        let refs = references(&[concat!(
            "pub fn go() {}\n",
            "#[cfg(test)]\nmod tests {\n use dev_only::assert_ok;\n",
            " #[test] fn t() { assert_ok(); }\n}\n",
        )]);
        let manifest = package(vec![dev_dependency("dev_only")]);
        assert!(misplaced(&manifest, &refs).0.is_empty());
    }

    /// The gate does not have to be a bare `cfg(test)`, and it does not have
    /// to sit on a module: anything that can only hold in a test build moves
    /// the code under it.
    #[test]
    fn any_gate_that_implies_test_moves_the_code_under_it() {
        let refs =
            references(&["#[cfg(all(test, unix))]\nfn helper() { dev_only::assert_ok(); }\n"]);
        let manifest = package(vec![dev_dependency("dev_only")]);
        assert!(misplaced(&manifest, &refs).0.is_empty());

        // ...and the gate has to actually imply it. `not(test)` is runtime
        // code, so a normal entry named there stays where it is.
        let refs = references(&["#[cfg(not(test))]\nfn helper() { runtime_only::go(); }\n"]);
        let manifest = package(vec![dependency("runtime_only")]);
        assert!(misplaced(&manifest, &refs).0.is_empty());
    }

    /// Doc examples are compiled as doctests, which link the dev-dependencies
    /// — and a word in a doc comment names no target at all. Either way it
    /// cannot place an entry, in any direction.
    #[test]
    fn a_mention_in_a_doc_comment_places_nothing() {
        let source = "/// ```\n/// use doc_crate::helper;\n/// ```\npub fn go() {}\n";
        let refs = references(&[source]);
        assert!(
            misplaced(&package(vec![dev_dependency("doc_crate")]), &refs)
                .0
                .is_empty(),
            "a dev-dependency named only by a doctest is correctly placed"
        );
        assert!(
            misplaced(&package(vec![dependency("doc_crate")]), &refs)
                .0
                .is_empty(),
            "and the same mention cannot move a normal entry either"
        );
    }

    /// Macro input and attribute arguments keep a dependency alive for the
    /// unused check. They cannot place one: we do not expand macros, so we do
    /// not know what code the mention ends up in.
    #[test]
    fn a_mention_through_a_macro_or_an_attribute_places_nothing() {
        let refs = references_from(&[
            ("lib", "pub fn go() {}\n"),
            (
                "test",
                concat!(
                    "#[attr_crate(with = \"string_crate::codec\")]\nstruct Wired;\n",
                    "fn t() { println!(\"{}\", macro_body_crate::VALUE); }\n",
                ),
            ),
        ]);
        let manifest = package(vec![
            dependency("attr_crate"),
            dependency("string_crate"),
            dependency("macro_body_crate"),
        ]);
        assert!(
            misplaced(&manifest, &refs).0.is_empty(),
            "an unattributable mention proves a use, not a place"
        );
    }

    /// A file no `mod` declaration names is compiled by whatever macro expands
    /// into its declaration, and that macro is the only thing that knows which
    /// target that is.
    #[test]
    fn a_mention_in_an_unreached_source_places_nothing() {
        let mut refs = references_from(&[("lib", "pub fn go() {}\n")]);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deps/tests");
        refs.add_unreached_sources(&dir, &HashSet::new());
        let manifest = package(vec![dependency("regression_only_crate")]);
        assert!(
            misplaced(&manifest, &refs).0.is_empty(),
            "the fixture file naming it is reached by `automod::dir!` alone"
        );
    }

    #[test]
    fn a_build_dependency_the_build_script_names_is_correctly_placed() {
        let refs = references_from(&[
            ("lib", "pub fn go() {}\n"),
            ("custom-build", "fn main() { cc::Build::new(); }\n"),
        ]);
        let manifest = package(vec![build_dependency("cc")]);
        assert!(misplaced(&manifest, &refs).0.is_empty());
    }

    /// The other direction: a `[build-dependencies]` entry the build script
    /// never touches is in a table nothing reads, and the code that does name
    /// it says which table that should have been.
    #[test]
    fn a_build_dependency_the_build_script_never_names_is_reported() {
        let refs = references_from(&[
            ("lib", "pub fn go() { stale::helper(); }\n"),
            ("custom-build", "fn main() {}\n"),
        ]);
        let manifest = package(vec![build_dependency("stale")]);
        assert_eq!(
            misplaced(&manifest, &refs).0,
            vec![("stale".to_string(), DependencyKind::Normal)]
        );

        let refs = references_from(&[
            ("test", "fn t() { stale::helper(); }\n"),
            ("custom-build", "fn main() {}\n"),
        ]);
        assert_eq!(
            misplaced(&manifest, &refs).0,
            vec![("stale".to_string(), DependencyKind::Development)],
            "the table it belongs in follows the code that names it"
        );
    }

    /// `include!` splices a file into the *item* that includes it, so the
    /// included code inherits that item's gate: a file pulled in from inside a
    /// `#[cfg(test)]` module is test code, not library code.
    #[test]
    fn an_included_file_inherits_the_context_of_the_include_site() {
        let mut refs = CrateReferences::default();
        // `deps.rs` is next to the probe and certainly names `proc_macro2`;
        // only the directory the path resolves against matters here.
        add_one(
            &mut refs,
            "lib",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs"),
            syn::parse_file("#[cfg(test)]\nmod tests {\n include!(\"deps.rs\");\n}\n").ok(),
        );
        // A *normal* entry is the discriminating case: reading the included
        // file as library code would leave this unreported, which is exactly
        // the answer a correctly-placed entry gets.
        let manifest = package(vec![dependency("proc-macro2")]);
        let (found, warnings) = misplaced(&manifest, &refs);
        assert!(
            warnings.is_empty(),
            "the include was followed: {warnings:?}"
        );
        assert_eq!(
            found,
            vec![("proc-macro2".to_string(), DependencyKind::Development)],
            "the only code naming it is behind `#[cfg(test)]`"
        );
    }

    /// A proc-macro crate's lib target is runtime code like any other: it is
    /// the only lib such a package has, and a stale `[build-dependencies]`
    /// entry there belongs in `[dependencies]`, not `[dev-dependencies]`.
    #[test]
    fn a_proc_macro_target_is_runtime_code() {
        let refs = references_from(&[
            ("proc-macro", "pub fn derive() { stale::helper(); }\n"),
            ("custom-build", "fn main() {}\n"),
        ]);
        let manifest = package(vec![build_dependency("stale")]);
        assert_eq!(
            misplaced(&manifest, &refs).0,
            vec![("stale".to_string(), DependencyKind::Normal)]
        );
    }

    /// An entry both shipping code and tests name is where it belongs, and an
    /// entry nothing names at all is the unused check's business — "no target
    /// of the right kind names it" is a weaker claim than "the table is wrong".
    #[test]
    fn an_entry_is_placed_only_on_positive_evidence() {
        let refs = references_from(&[
            ("lib", "pub fn go() { shared::helper(); }\n"),
            ("test", "fn t() { shared::helper(); }\n"),
        ]);
        let manifest = package(vec![dependency("shared"), dependency("named_by_nothing")]);
        assert!(misplaced(&manifest, &refs).0.is_empty());
    }

    /// A dev-dependency the library itself appears to name is never reported.
    /// Such a manifest does not compile, so the likelier explanation is that
    /// the mention was attributed to the wrong code — a `#[cfg(test)] mod
    /// tests;` in its own file, say, whose gate lives in the parent.
    #[test]
    fn a_dev_dependency_named_by_runtime_code_is_never_reported() {
        let refs = references(&["pub fn go() { dev_only::helper(); }\n"]);
        let manifest = package(vec![dev_dependency("dev_only")]);
        assert!(misplaced(&manifest, &refs).0.is_empty());
    }

    /// The allowlist means "do not judge this entry", which covers where it is
    /// declared as much as whether anything names it.
    #[test]
    fn an_allowlisted_entry_is_never_placed() {
        let refs = references_from(&[
            ("lib", "pub fn go() {}\n"),
            ("test", "fn t() { test_helper::assert_ok(); }\n"),
        ]);
        let manifest = package(vec![dependency("test_helper")]);
        let allowed = DependencyAllowList::for_tests(&["test_helper"]);
        assert!(misplaced_allowing(&manifest, &refs, &allowed).0.is_empty());
    }

    /// Code we cannot see could name the entry from anywhere, so the placement
    /// question is as unanswerable as the unused one — and says so.
    #[test]
    fn hidden_code_skips_the_placement_check_out_loud() {
        let refs = references(&["include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n"]);
        let manifest = package(vec![dependency("test_helper")]);
        let (found, warnings) = misplaced(&manifest, &refs);
        assert!(found.is_empty());
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("misplaced-dependency check skipped")),
            "the skip must be surfaced: {warnings:?}"
        );
    }
}
