//! `cfg` gate evaluation.
//!
//! Every phase before this one made Deadwood resolve better or report less.
//! This one is the first that can *invent* a finding, so the shape of the
//! answer matters more than the answer itself.
//!
//! # A matrix, not a configuration
//!
//! Deadwood does not analyze one build. It analyzes a *set* of builds — by
//! default every feature combination and every target — and a gate is followed
//! whenever it holds in at least one of them. That is exactly the pre-`cfg`
//! behavior (`#[cfg(windows)] mod win;` is followed on Linux, because some
//! build compiles it), and it is what keeps an absent `[cfg]` section a no-op.
//!
//! So a predicate evaluates to one of three answers ([`Truth`]): it holds in
//! every configuration the matrix admits, in none of them, or in some but not
//! others. A gate we cannot evaluate at all — `cfg(accessible(..))`,
//! `cfg(docsrs)`, a `cfg_attr` indirection — answers [`Truth::Either`], which
//! is the same answer as "sometimes" for every decision made from it, and
//! means the gated code is analyzed as it always was.
//!
//! The combinators lose correlation between atoms:
//! `all(feature = "a", not(feature = "a"))` is `Either.and(Either)`, so it
//! reads as satisfiable when it provably is not. That direction is the safe
//! one — it costs a finding rather than inventing one — and tracking
//! correlation would mean a SAT solver for a payoff nobody has asked for.
//!
//! # Two questions, two matrices
//!
//! Two different things are decided here, against two different matrices, and
//! keeping them apart is what makes the phase safe:
//!
//! 1. **Is this code part of the analyzed configuration?** Judged against the
//!    matrix the user configured ([`Gates::compiled`]). Code the matrix rules
//!    out is not read, not resolved, and not reported dead — it is simply not
//!    in the build being analyzed. With the default matrix nothing is ever
//!    ruled out, so this changes nothing until a `deadwood.toml` says so.
//! 2. **Can this gate hold in *any* build at all?** Judged against the maximal
//!    matrix — every feature the manifest declares, every target, test and
//!    non-test ([`Gates::gate_sites`]). A `mod` behind a feature no manifest
//!    declares is dead by construction, in every checkout, on every platform.
//!    That is a finding, and it is the strongest reason this phase exists.
//!
//! The two are deliberately not the same test. An impossible gate is
//! *reported*, not hidden: dropping the code behind it would change what every
//! other detector sees, and a new finding class must not quietly move the
//! others.
//!
//! # What is evaluated
//!
//! - `feature = "..."` against the features `cargo metadata` reports for the
//!   package, which includes the implicit feature Cargo synthesizes for each
//!   optional dependency.
//! - `test`, which the matrix admits by default (see `docs/HISTORY.md` phase 4
//!   for why the quiet default was chosen).
//! - `target_os = "..."`, `target_family = "..."`, `unix`, `windows`.
//! - `not`, `all`, `any` over any of those, at any nesting depth.
//!
//! Everything else — `target_arch`, `target_env`, `debug_assertions`,
//! `miri`, a bare `cfg` a build script sets — is [`Truth::Either`]. The matrix
//! has no axis for them, so no configuration is ruled out by one, and the
//! answer is honest rather than guessed.
//!
//! # One question here is not about `cfg` at all
//!
//! [`Gates::test_only`] asks whether an item is confined to a test build, and
//! `#[cfg(test)]` is not the only way to write that: `#[test]` confines the
//! function it sits on just as completely, and it is not a `cfg`, carries no
//! predicate, and is judged against no matrix. It is answered here anyway,
//! because it is the same question with the same callers — and deliberately
//! *not* answered in [`eval`] or [`prune`], which read configuration
//! predicates. See [`Gates::test_only`] and [`Site`] for the boundary, which is
//! narrower than the one [`crate::resolve`] draws around the same two
//! attributes for a different question.

use std::collections::{HashMap, HashSet};

use syn::punctuated::Punctuated;

use crate::metadata::Package;

/// What a `cfg` predicate evaluates to across every configuration a matrix
/// admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    /// Holds in every configuration.
    Always,
    /// Holds in none of them: the code behind it is never compiled.
    Never,
    /// Holds in some and not others — or cannot be evaluated at all, which
    /// leads to the same decision everywhere it is used.
    Either,
}

impl Truth {
    fn not(self) -> Truth {
        match self {
            Truth::Always => Truth::Never,
            Truth::Never => Truth::Always,
            Truth::Either => Truth::Either,
        }
    }

    fn and(self, other: Truth) -> Truth {
        match (self, other) {
            (Truth::Never, _) | (_, Truth::Never) => Truth::Never,
            (Truth::Always, Truth::Always) => Truth::Always,
            _ => Truth::Either,
        }
    }

    fn or(self, other: Truth) -> Truth {
        match (self, other) {
            (Truth::Always, _) | (_, Truth::Always) => Truth::Always,
            (Truth::Never, Truth::Never) => Truth::Never,
            _ => Truth::Either,
        }
    }
}

/// The set of builds Deadwood analyzes, from the `[cfg]` section of
/// `deadwood.toml`.
///
/// [`Matrix::default`] is the union of every possibility, which is the
/// behavior of a Deadwood with no `cfg` evaluation at all: every feature may
/// be on or off, every target is possible, and `#[cfg(test)]` code is part of
/// the build.
///
/// Equality is over the builds a matrix admits, which is what the config tests
/// need to assert about a parsed file.
#[derive(Debug, PartialEq, Eq)]
pub struct Matrix {
    /// Feature names to analyze as enabled, closed over the features they
    /// enable in each package. `None` means every feature may be on or off.
    features: Option<Vec<String>>,
    /// `target_os` values to analyze. `None` means every target is possible.
    target_os: Option<HashSet<String>>,
    /// Whether `#[cfg(test)]` code is part of the analyzed build.
    test: bool,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix {
            features: None,
            target_os: None,
            test: true,
        }
    }
}

impl Matrix {
    /// Build a matrix from the raw config values, where `None` means "not
    /// narrowed" for each axis.
    pub(crate) fn new(
        features: Option<Vec<String>>,
        target_os: Option<Vec<String>>,
        test: Option<bool>,
    ) -> Matrix {
        Matrix {
            features,
            target_os: target_os.map(|values| values.into_iter().collect()),
            test: test.unwrap_or(true),
        }
    }
}

/// What kind of item an attribute set sits on, for the one question where that
/// matters: rustc honours `#[test]` on a free function and nowhere else.
///
/// Verified against rustc rather than assumed. `#[test] mod tests { .. }` and
/// `#[test]` on an associated function are `error: the #[test] attribute may
/// only be used on a free function`; on a macro invocation
/// (`#[test] include!("gen.rs")`) it is the same message as a *warning* and the
/// item is compiled into the library regardless. So a test attribute written
/// anywhere but a `fn` confines nothing, and a caller asking about anything
/// else says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// A `fn` item: at module scope, or inside another function's body, where
    /// rustc still strips it from every non-test build (it only declines to
    /// register it with the harness — `warning: cannot test inner items`).
    FreeFn,
    /// Anything else — a `mod`, an associated or trait function, an `extern`
    /// block member, a macro invocation, a file's own inner attributes — where
    /// only a `cfg` gate can confine the item.
    Other,
}

/// The built-in attributes that confine a function to a test build on their
/// own, with no `cfg` and no predicate.
///
/// `#[bench]` is here with `#[test]` for the reason
/// [`crate::resolve`]'s root set pairs them: `cargo bench` is no more a
/// consumer of the crate than `cargo test` is, and rustc strips a `#[bench] fn`
/// from a non-test build identically (it is unstable, so such a crate is
/// nightly-only, which changes nothing about where the function is compiled).
const TEST_BUILD_ATTRS: &[&str] = &["test", "bench"];

/// Whether `attrs` hold a built-in test attribute.
///
/// The match is exact where [`crate::resolve`]'s deliberately is not, and the
/// asymmetry is the point: there, treating `#[tokio::test]` as a test entry
/// point can only keep an item alive, so the *last* path segment decides. Here
/// the answer moves a mention out of the library and into the tests, which is
/// what makes a `[dependencies]` entry reportable — so only the attribute rustc
/// itself expands counts: a bare, single-segment `test` or `bench` with no
/// arguments.
///
/// A test attribute reached through `cfg_attr` is not matched either, for the
/// reason [`eval_attrs`] does not follow that indirection at all: the attribute
/// it would expand to is not written in the syntax. Like every other refusal
/// here, that leaves the function's mentions attributed to the code they are
/// written in.
///
/// Everything else is a guess about a macro Deadwood cannot expand, and the
/// guess would be wrong in both directions: `#[tokio::test]` does confine (it
/// expands to the built-in attribute), while an attribute macro merely *named*
/// `test` need not. `#[core::prelude::v1::test]` — which is what
/// `#[tokio::test]` expands to, and which rustc does honour — is not matched
/// either, because nothing distinguishes it from the proc-macro case before
/// expansion. Neither refusal leaves the function's mentions attributed as
/// library code any more: both spellings are an attribute macro to
/// [`unexpandable_macro`], which hands the item to the opaque context instead
/// of guessing ([#49](https://github.com/rlorenzo/deadwood/issues/49)).
fn test_attribute(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        matches!(attr.meta, syn::Meta::Path(_))
            && TEST_BUILD_ATTRS
                .iter()
                .any(|name| attr.path().is_ident(name))
    })
}

/// The attributes rustc itself defines, which are inert here: none of them is
/// a macro, so none can rewrite the item it sits on or move it into another
/// build. Taken from the Reference's built-in attributes index.
///
/// `unsafe` is on the list for the `#[unsafe(no_mangle)]` wrapper syntax,
/// whose contents can only be built-in attributes.
///
/// A built-in attribute this list is missing is read as an attribute macro,
/// which makes its item opaque — that costs a placement claim and cannot
/// invent one, so a stabilization this list has not caught up with degrades in
/// the direction the project prefers.
const BUILT_IN_ATTRIBUTES: &[&str] = &[
    "allow",
    "automatically_derived",
    "bench",
    "cfg",
    "cfg_attr",
    "cold",
    "collapse_debuginfo",
    "coverage",
    "crate_name",
    "crate_type",
    "debugger_visualizer",
    "deny",
    "deprecated",
    "derive",
    "doc",
    "expect",
    "export_name",
    "feature",
    "forbid",
    "global_allocator",
    "ignore",
    "inline",
    "instruction_set",
    "link",
    "link_name",
    "link_ordinal",
    "link_section",
    "macro_export",
    "macro_use",
    "must_use",
    "naked",
    "no_builtins",
    "no_implicit_prelude",
    "no_link",
    "no_main",
    "no_mangle",
    "no_std",
    "non_exhaustive",
    "panic_handler",
    "path",
    "proc_macro",
    "proc_macro_attribute",
    "proc_macro_derive",
    "recursion_limit",
    "repr",
    "should_panic",
    "target_feature",
    "test",
    "track_caller",
    "type_length_limit",
    "unsafe",
    "used",
    "warn",
    "windows_subsystem",
];

/// The namespaces rustc reserves for external tools. An attribute under one —
/// `#[rustfmt::skip]`, `#[clippy::cognitive_complexity]`,
/// `#[diagnostic::on_unimplemented]` — is metadata for that tool, not a macro:
/// the item is compiled exactly as written.
const TOOL_NAMESPACES: &[&str] = &["clippy", "diagnostic", "rustfmt"];

/// Whether `attrs` put their item in the hands of an attribute macro Deadwood
/// cannot expand.
///
/// Such a macro receives the whole item as tokens and may emit anything in its
/// place — the item verbatim, the item confined to a test build
/// (`#[tokio::test]`), or nothing at all. What survives, and in which build,
/// is unknowable before expansion, which is precisely what
/// `crate::deps::Contexts::OPAQUE` exists to say; [`crate::deps`] moves the
/// item's mentions there rather than reading them as the code they
/// syntactically sit in.
///
/// What is *not* a macro is decided here, and each exclusion is an attribute
/// kind that rewrites nothing:
///
/// - A single-segment attribute on the built-in list
///   ([`BUILT_IN_ATTRIBUTES`]).
/// - A path under a reserved tool namespace ([`TOOL_NAMESPACES`]).
/// - A single-segment attribute on an item that also carries `#[derive(..)]`.
///   A derive helper (`#[serde(rename_all = "..")]`) is only legal beside its
///   derive, is inert, and its name is registered by a macro Deadwood cannot
///   expand — so it cannot be told from an attribute macro by spelling. Reading
///   it as a helper keeps a `#[derive]`-carrying struct's mentions placeable;
///   the cost is an attribute macro sharing an item with a derive, which is
///   read as a helper and leaves the item attributed as written.
///
/// Everything else is a macro: a multi-segment path that is not a tool's
/// (`#[tokio::test]`, `#[core::prelude::v1::test]`), and a single-segment name
/// that is not built in and has no derive to belong to (`#[rstest]` brought in
/// by `use`) — on stable rustc that spelling cannot be anything but an
/// attribute macro in scope.
///
/// An attribute reached through `cfg_attr` is not examined, for the reason
/// [`test_attribute`] and [`eval_attrs`] do not follow that indirection — and
/// here the refusal is also simply correct: in every build whose predicate
/// does not hold, the item is compiled exactly as written, so its mentions are
/// attributable to the code they sit in.
pub(crate) fn unexpandable_macro(attrs: &[syn::Attribute]) -> bool {
    let derived = attrs.iter().any(|attr| attr.path().is_ident("derive"));
    attrs.iter().any(|attr| {
        let path = attr.path();
        if path.get_ident().is_some() {
            !derived && !BUILT_IN_ATTRIBUTES.iter().any(|known| path.is_ident(known))
        } else {
            let head = &path.segments[0].ident;
            !TOOL_NAMESPACES.iter().any(|tool| head == tool)
        }
    })
}

/// A [`Matrix`] resolved against one package's manifest.
///
/// Features are per package, so the same `#[cfg(feature = "std")]` is a
/// different question in each — which is also why a file shared between two
/// packages is only reported when *every* package that compiles it agrees the
/// gate is impossible (see `lib.rs`).
pub struct Gates<'a> {
    matrix: &'a Matrix,
    /// Every feature name the manifest declares, including the implicit
    /// feature Cargo synthesizes for an optional dependency.
    declared: HashSet<String>,
    /// The features the configured matrix turns on, closed over the features
    /// they enable. `None` when the matrix does not narrow features.
    enabled: Option<HashSet<String>>,
    /// The optional dependencies that same closure activates. `None` when the
    /// matrix does not narrow features, where every one of them is possible.
    enabled_dependencies: Option<HashSet<String>>,
}

impl<'a> Gates<'a> {
    /// Resolve `matrix` against `package`'s feature table.
    pub fn new(matrix: &'a Matrix, package: &Package) -> Gates<'a> {
        let mut declared: HashSet<String> = package.features.keys().cloned().collect();
        // Cargo synthesizes a feature per optional dependency and reports it
        // in `features`, but only when nothing else claims the name; taking
        // the manifest keys too costs nothing and cannot be wrong.
        for dependency in &package.dependencies {
            if dependency.optional {
                declared.insert(dependency.manifest_name().to_string());
            }
        }

        let (enabled, enabled_dependencies) = match &matrix.features {
            None => (None, None),
            Some(on) => {
                let (features, dependencies) = close_over(&package.features, on);
                (Some(features), Some(dependencies))
            }
        };

        Gates {
            matrix,
            declared,
            enabled,
            enabled_dependencies,
        }
    }

    /// The matrix as configured: what "is this code compiled?" is judged
    /// against.
    fn configured(&self) -> World<'_> {
        World {
            declared: &self.declared,
            enabled: self.enabled.as_ref(),
            target_os: self.matrix.target_os.as_ref(),
            test: self.matrix.test,
        }
    }

    /// Every build that could ever exist: what "can this gate hold at all?" is
    /// judged against.
    fn maximal(&self) -> World<'_> {
        World {
            declared: &self.declared,
            enabled: None,
            target_os: None,
            test: true,
        }
    }

    /// Whether the item carrying `attrs` belongs to the analyzed build.
    ///
    /// False only when the configured matrix rules it out *and* the gate could
    /// hold in some other build — that is, when the user's own narrowing is
    /// what excludes it. A gate that can never hold anywhere is reported by
    /// [`Gates::gate_sites`] and left in place, so that adding a finding kind
    /// never moves what the other detectors see.
    pub fn compiled(&self, attrs: &[syn::Attribute]) -> bool {
        if eval_attrs(attrs, &self.configured()) != Truth::Never {
            return true;
        }
        eval_attrs(attrs, &self.maximal()) == Truth::Never
    }

    /// Whether `attrs` confine the item to a test build: it is compiled by
    /// some build of the package, and by none that is not a test build.
    ///
    /// Two spellings confine an item, and only one of them is a gate:
    ///
    /// - `#[cfg(test)]` and everything that implies it
    ///   (`#[cfg(all(test, unix))]`), judged against the maximal matrix rather
    ///   than the configured one — the question is a property of the code, not
    ///   of what the user asked to analyze.
    /// - `#[test]` and `#[bench]`, which carry no predicate at all: rustc moves
    ///   the function they sit on into the test harness binary and leaves it
    ///   out of every other build. That is *unconditional* confinement, so it
    ///   contributes to the "no non-test build compiles this" half of the
    ///   answer exactly as `cfg(test)` does — and `site` is what says whether
    ///   rustc honours it here at all ([`Site`]).
    ///
    /// An item behind a gate that can hold in *no* build answers `false`
    /// whichever way it is written: it is dead by construction
    /// ([`Gates::gate_sites`] reports it), not test-only. `#[test]
    /// #[cfg(feature = "nope")] fn` is compiled by nothing, and calling it
    /// test-only would attribute its mentions to a test build that does not
    /// exist either.
    ///
    /// [`crate::deps`] uses this to tell a dev-dependency used by the unit
    /// tests inside a library from one the library itself depends on;
    /// [`crate::modtree`] uses it on `mod` declarations and `include!` sites,
    /// which are [`Site::Other`] and so unaffected by the test attributes.
    pub fn test_only(&self, attrs: &[syn::Attribute], site: Site) -> bool {
        let mut non_test = self.maximal();
        non_test.test = false;
        let confined = eval_attrs(attrs, &non_test) == Truth::Never
            || (site == Site::FreeFn && test_attribute(attrs));
        confined && eval_attrs(attrs, &self.maximal()) != Truth::Never
    }

    /// The features named by `gate` that the manifest does not declare, when
    /// the gate can hold in no build at all.
    fn verdict(&self, gate: &syn::Meta) -> Verdict {
        if eval(gate, &self.maximal()) != Truth::Never {
            return Verdict::CanHold;
        }
        let mut undeclared = Vec::new();
        collect_undeclared(gate, &self.declared, &mut undeclared);
        undeclared.sort();
        undeclared.dedup();
        Verdict::Impossible { undeclared }
    }

    /// Whether the configured matrix can turn the optional dependency `entry`
    /// on, `entry` being its manifest key.
    ///
    /// With features unnarrowed every optional dependency is possible, which
    /// is what makes them judgeable at all: the code behind
    /// `#[cfg(feature = "...")]` is analyzed, so a reference to the crate is
    /// found wherever one exists.
    pub fn optional_dependency_possible(&self, entry: &str) -> bool {
        match &self.enabled_dependencies {
            None => true,
            Some(enabled) => enabled.contains(entry),
        }
    }

    /// What the platform key of a `[target.'...'.dependencies]` table means
    /// for the entries in it.
    ///
    /// The key is either a `cfg(...)` expression or a bare target triple;
    /// triples are not modelled, so one is always [`TargetVerdict::Possible`]
    /// and its entries are judged like any other.
    pub fn target_expression(&self, key: &str) -> TargetVerdict {
        let predicate = key
            .strip_prefix("cfg(")
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|inner| inner.parse::<proc_macro2::TokenStream>().ok())
            .and_then(|tokens| predicates(&tokens))
            .and_then(|parsed| match &parsed[..] {
                [only] => Some(only.clone()),
                _ => None,
            });
        let Some(predicate) = predicate else {
            return TargetVerdict::Possible;
        };
        if eval(&predicate, &self.configured()) != Truth::Never {
            TargetVerdict::Possible
        } else if eval(&predicate, &self.maximal()) == Truth::Never {
            TargetVerdict::NeverBuilt
        } else {
            TargetVerdict::RuledOutByMatrix
        }
    }

    /// Every `#[cfg]` gate on a module-level item of `ast`, with the verdict on
    /// whether it can hold in any build at all.
    ///
    /// Items behind an impossible gate are not descended into: the outermost
    /// gate is the one to fix, and everything under it is dead for the same
    /// single reason.
    pub fn gate_sites(&self, ast: &syn::File) -> Vec<GateSite> {
        let mut sites = Vec::new();
        // An inner `#![cfg(...)]` gates the file it is written in. If it can
        // never hold, every item below it is dead for that one reason, so the
        // file-level gate is the only one worth naming.
        if self.walk_attrs(&ast.attrs, None, &mut sites) {
            return sites;
        }
        self.walk_items(&ast.items, &mut sites);
        sites
    }

    fn walk_items(&self, items: &[syn::Item], sites: &mut Vec<GateSite>) {
        for item in items {
            if self.walk_attrs(attrs_of(item), item_name(item), sites) {
                continue;
            }
            match item {
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        self.walk_items(inner, sites);
                    }
                }
                syn::Item::Impl(i) => self.walk_nested(
                    i.items
                        .iter()
                        .map(|it| (impl_item_attrs(it), impl_item_name(it))),
                    sites,
                ),
                syn::Item::Trait(t) => self.walk_nested(
                    t.items
                        .iter()
                        .map(|it| (trait_item_attrs(it), trait_item_name(it))),
                    sites,
                ),
                syn::Item::ForeignMod(f) => self.walk_nested(
                    f.items
                        .iter()
                        .map(|it| (foreign_item_attrs(it), foreign_item_name(it))),
                    sites,
                ),
                _ => {}
            }
        }
    }

    fn walk_nested<'i>(
        &self,
        members: impl Iterator<Item = (&'i [syn::Attribute], Option<String>)>,
        sites: &mut Vec<GateSite>,
    ) {
        for (attrs, name) in members {
            self.walk_attrs(attrs, name, sites);
        }
    }

    /// Record a site per `#[cfg]` attribute; answer whether any of them makes
    /// the item dead by construction.
    fn walk_attrs(
        &self,
        attrs: &[syn::Attribute],
        name: Option<String>,
        sites: &mut Vec<GateSite>,
    ) -> bool {
        let mut impossible = false;
        for attr in attrs {
            let Some(gate) = cfg_predicate(attr) else {
                continue;
            };
            let verdict = self.verdict(&gate);
            impossible |= matches!(verdict, Verdict::Impossible { .. });
            sites.push(GateSite {
                line: attribute_line(attr),
                name: name.clone(),
                gate: render(&gate),
                verdict,
            });
        }
        impossible
    }
}

/// A `#[cfg]` gate found on an item, and what became of it.
pub struct GateSite {
    /// Line of the `#[cfg]` attribute itself, which is what a reader has to
    /// delete.
    pub line: usize,
    /// The gated item's name, when it has one (`impl` and `use` do not).
    pub name: Option<String>,
    /// The predicate as written, e.g. `feature = "nope"`.
    pub gate: String,
    pub verdict: Verdict,
}

/// What the `cfg` key of a `[target.'...'.dependencies]` table says about the
/// entries under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetVerdict {
    /// Some target the matrix admits builds them, so they can be judged like
    /// any other entry.
    Possible,
    /// The configured matrix leaves out every target that would build them,
    /// so the code that could name them was never read.
    RuledOutByMatrix,
    /// No target builds them, on any matrix: `cfg(any())` is the idiom for an
    /// entry that exists to constrain version resolution and is deliberately
    /// never compiled (serde pins `serde_derive` this way).
    NeverBuilt,
}

/// Whether a gate can hold in any build at all.
pub enum Verdict {
    CanHold,
    /// Dead by construction. `undeclared` names the features the manifest does
    /// not declare, which is the usual reason and the actionable part.
    Impossible {
        undeclared: Vec<String>,
    },
}

/// Remove every item the configured matrix leaves out of the analyzed build.
///
/// Pruning the AST rather than teaching each detector about `cfg` is what
/// keeps this phase small: usage resolution and the dependency collector simply never
/// see the items, so excluded code stops counting as a use and stops keeping a
/// dependency alive, with no plumbing of their own. With the default matrix
/// nothing is ever removed.
pub fn prune(gates: &Gates<'_>, file: &mut syn::File) {
    // An inner `#![cfg(...)]` the matrix rules out takes the whole file with
    // it. Module resolution catches this first and never hands such a file
    // here, but the function has to be right on its own terms.
    if !gates.compiled(&file.attrs) {
        file.items.clear();
        return;
    }
    prune_items(gates, &mut file.items);
}

fn prune_items(gates: &Gates<'_>, items: &mut Vec<syn::Item>) {
    items.retain(|item| gates.compiled(attrs_of(item)));
    for item in items {
        match item {
            syn::Item::Mod(m) => {
                if let Some((_, inner)) = &mut m.content {
                    prune_items(gates, inner);
                }
            }
            syn::Item::Impl(i) => i.items.retain(|it| gates.compiled(impl_item_attrs(it))),
            syn::Item::Trait(t) => t.items.retain(|it| gates.compiled(trait_item_attrs(it))),
            syn::Item::ForeignMod(f) => {
                f.items.retain(|it| gates.compiled(foreign_item_attrs(it)));
            }
            _ => {}
        }
    }
}

// -- evaluation ------------------------------------------------------------

/// One matrix, flattened to the axes evaluation actually reads.
struct World<'a> {
    declared: &'a HashSet<String>,
    /// `None` when features are not narrowed, i.e. any of them may be on.
    enabled: Option<&'a HashSet<String>>,
    /// `None` when targets are not narrowed.
    target_os: Option<&'a HashSet<String>>,
    test: bool,
}

/// The conjunction of every `#[cfg]` attribute on one item.
///
/// Attributes that are not `cfg` — `cfg_attr` included, since following its
/// indirection is out of scope — constrain nothing, so they leave the result
/// alone and the item keeps being analyzed.
fn eval_attrs(attrs: &[syn::Attribute], world: &World<'_>) -> Truth {
    let mut truth = Truth::Always;
    for attr in attrs {
        if let Some(predicate) = cfg_predicate(attr) {
            truth = truth.and(eval(&predicate, world));
        }
    }
    truth
}

fn eval(meta: &syn::Meta, world: &World<'_>) -> Truth {
    match meta {
        syn::Meta::Path(path) => match single_ident(path).as_deref() {
            Some("test") => {
                if world.test {
                    Truth::Either
                } else {
                    // Only the non-test build is analyzed, so `cfg(test)` holds
                    // nowhere in it — and `cfg(not(test))` holds everywhere.
                    Truth::Never
                }
            }
            Some("unix") => family_truth(world, Family::Unix),
            Some("windows") => family_truth(world, Family::Windows),
            _ => Truth::Either,
        },
        syn::Meta::NameValue(nv) => {
            let Some(value) = string_literal(&nv.value) else {
                return Truth::Either;
            };
            match single_ident(&nv.path).as_deref() {
                Some("feature") => feature_truth(world, &value),
                Some("target_os") => target_os_truth(world, &value),
                Some("target_family") => match value.as_str() {
                    "unix" => family_truth(world, Family::Unix),
                    "windows" => family_truth(world, Family::Windows),
                    _ => Truth::Either,
                },
                _ => Truth::Either,
            }
        }
        syn::Meta::List(list) => {
            let Some(inner) = predicates(&list.tokens) else {
                return Truth::Either;
            };
            match single_ident(&list.path).as_deref() {
                // `not` takes exactly one predicate; anything else is not the
                // `not` we know how to read.
                Some("not") => match &inner[..] {
                    [only] => eval(only, world).not(),
                    _ => Truth::Either,
                },
                Some("all") => inner
                    .iter()
                    .fold(Truth::Always, |acc, meta| acc.and(eval(meta, world))),
                Some("any") => inner
                    .iter()
                    .fold(Truth::Never, |acc, meta| acc.or(eval(meta, world))),
                _ => Truth::Either,
            }
        }
    }
}

fn feature_truth(world: &World<'_>, name: &str) -> Truth {
    // The finding this whole phase is for: no build of this package can turn
    // on a feature its manifest never declares.
    if !world.declared.contains(name) {
        return Truth::Never;
    }
    match world.enabled {
        None => Truth::Either,
        Some(enabled) => {
            if enabled.contains(name) {
                Truth::Always
            } else {
                Truth::Never
            }
        }
    }
}

fn target_os_truth(world: &World<'_>, value: &str) -> Truth {
    let Some(selected) = world.target_os else {
        return Truth::Either;
    };
    if !selected.contains(value) {
        Truth::Never
    } else if selected.len() == 1 {
        Truth::Always
    } else {
        Truth::Either
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Unix,
    Windows,
    /// A target in neither family: `wasi`, `none`, `uefi`, and friends.
    Neither,
}

fn family_truth(world: &World<'_>, wanted: Family) -> Truth {
    let Some(selected) = world.target_os else {
        return Truth::Either;
    };
    let (mut inside, mut outside) = (false, false);
    for os in selected {
        match belongs_to(os, wanted) {
            Some(true) => inside = true,
            Some(false) => outside = true,
            // An OS we do not recognize could be in either family, and
            // guessing would be the one mistake this phase must not make.
            None => return Truth::Either,
        }
    }
    match (inside, outside) {
        (true, false) => Truth::Always,
        (true, true) => Truth::Either,
        // Including the empty selection, which admits no target at all.
        (false, _) => Truth::Never,
    }
}

/// Whether an OS belongs to a family, or `None` when the name is not one we
/// know and the answer would be a guess.
///
/// The windows family is the one case that is never a guess, whatever the OS:
/// `target_family = "windows"` holds exactly when `target_os = "windows"`.
fn belongs_to(os: &str, family: Family) -> Option<bool> {
    if family == Family::Windows {
        return Some(os == "windows");
    }
    family_of(os).map(|known| known == family)
}

/// The target family of an OS name, or `None` when the name is not one we
/// know — where "could be either" is the only honest answer.
fn family_of(os: &str) -> Option<Family> {
    const UNIX: &[&str] = &[
        "aix",
        "android",
        "cygwin",
        "dragonfly",
        "emscripten",
        "freebsd",
        "haiku",
        "hurd",
        "illumos",
        "ios",
        "linux",
        "macos",
        "netbsd",
        "nto",
        "openbsd",
        "redox",
        "solaris",
        "tvos",
        "visionos",
        "watchos",
    ];
    const NEITHER: &[&str] = &[
        "hermit",
        "none",
        "psp",
        "solid_asp3",
        "uefi",
        "unknown",
        "wasi",
        "xous",
    ];
    if os == "windows" {
        Some(Family::Windows)
    } else if UNIX.contains(&os) {
        Some(Family::Unix)
    } else if NEITHER.contains(&os) {
        Some(Family::Neither)
    } else {
        None
    }
}

/// Every `feature = "..."` name in a gate that the manifest does not declare.
fn collect_undeclared(meta: &syn::Meta, declared: &HashSet<String>, out: &mut Vec<String>) {
    match meta {
        syn::Meta::Path(_) => {}
        syn::Meta::NameValue(nv) => {
            if single_ident(&nv.path).as_deref() == Some("feature")
                && let Some(name) = string_literal(&nv.value)
                && !declared.contains(&name)
            {
                out.push(name);
            }
        }
        syn::Meta::List(list) => {
            for inner in predicates(&list.tokens).unwrap_or_default() {
                collect_undeclared(&inner, declared, out);
            }
        }
    }
}

/// The features the matrix turns on, and the optional dependencies they
/// activate, closed over what each feature enables.
///
/// `features = ["default"]` has to mean everything `default` pulls in, or the
/// setting would be unusable; and an optional dependency is only judgeable
/// when some feature in that closure can turn it on.
fn close_over(
    table: &HashMap<String, Vec<String>>,
    requested: &[String],
) -> (HashSet<String>, HashSet<String>) {
    let mut features: HashSet<String> = HashSet::new();
    let mut dependencies: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = requested.to_vec();
    while let Some(feature) = queue.pop() {
        if !features.insert(feature.clone()) {
            continue;
        }
        for enabled in table.get(&feature).into_iter().flatten() {
            match enabled.split_once('/') {
                // `dep?/feature` forwards only if `dep` is already on, so it
                // is not what turns it on.
                Some((dependency, _)) => {
                    if !dependency.ends_with('?') {
                        dependencies.insert(dependency.to_string());
                    }
                }
                None => match enabled.strip_prefix("dep:") {
                    Some(dependency) => {
                        dependencies.insert(dependency.to_string());
                    }
                    None => queue.push(enabled.clone()),
                },
            }
        }
    }
    (features, dependencies)
}

// -- syntax ----------------------------------------------------------------

/// The single predicate of a `#[cfg(...)]` attribute, if this is one.
fn cfg_predicate(attr: &syn::Attribute) -> Option<syn::Meta> {
    let syn::Meta::List(list) = &attr.meta else {
        return None;
    };
    if single_ident(&list.path).as_deref() != Some("cfg") {
        return None;
    }
    // `#[cfg()]` gates nothing, and yields nothing here.
    predicates(&list.tokens)?.into_iter().next()
}

/// The comma-separated predicates inside a `cfg` combinator's parentheses.
fn predicates(tokens: &proc_macro2::TokenStream) -> Option<Vec<syn::Meta>> {
    syn::parse::Parser::parse2(
        Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        tokens.clone(),
    )
    .ok()
    .map(|parsed| parsed.into_iter().collect())
}

fn single_ident(path: &syn::Path) -> Option<String> {
    path.get_ident().map(ToString::to_string)
}

fn string_literal(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) => Some(text.value()),
        _ => None,
    }
}

/// The predicate as a reader would write it, for the finding message.
fn render(meta: &syn::Meta) -> String {
    match meta {
        syn::Meta::Path(path) => single_ident(path).unwrap_or_else(|| "…".to_string()),
        syn::Meta::NameValue(nv) => match (single_ident(&nv.path), string_literal(&nv.value)) {
            (Some(key), Some(value)) => format!("{key} = {value:?}"),
            _ => "…".to_string(),
        },
        syn::Meta::List(list) => {
            let key = single_ident(&list.path).unwrap_or_else(|| "…".to_string());
            let inner: Vec<String> = predicates(&list.tokens)
                .unwrap_or_default()
                .iter()
                .map(render)
                .collect();
            format!("{key}({})", inner.join(", "))
        }
    }
}

/// The line the `#[cfg]` attribute is written on.
fn attribute_line(attr: &syn::Attribute) -> usize {
    attr.path()
        .segments
        .first()
        .map_or(0, |segment| segment.ident.span().start().line)
}

pub(crate) fn attrs_of(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

fn item_name(item: &syn::Item) -> Option<String> {
    let ident = match item {
        syn::Item::Const(i) => &i.ident,
        syn::Item::Enum(i) => &i.ident,
        syn::Item::ExternCrate(i) => &i.ident,
        syn::Item::Fn(i) => &i.sig.ident,
        syn::Item::Mod(i) => &i.ident,
        syn::Item::Static(i) => &i.ident,
        syn::Item::Struct(i) => &i.ident,
        syn::Item::Trait(i) => &i.ident,
        syn::Item::TraitAlias(i) => &i.ident,
        syn::Item::Type(i) => &i.ident,
        syn::Item::Union(i) => &i.ident,
        _ => return None,
    };
    Some(ident.to_string())
}

pub(crate) fn impl_item_attrs(item: &syn::ImplItem) -> &[syn::Attribute] {
    match item {
        syn::ImplItem::Const(i) => &i.attrs,
        syn::ImplItem::Fn(i) => &i.attrs,
        syn::ImplItem::Type(i) => &i.attrs,
        syn::ImplItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn impl_item_name(item: &syn::ImplItem) -> Option<String> {
    match item {
        syn::ImplItem::Const(i) => Some(i.ident.to_string()),
        syn::ImplItem::Fn(i) => Some(i.sig.ident.to_string()),
        syn::ImplItem::Type(i) => Some(i.ident.to_string()),
        _ => None,
    }
}

pub(crate) fn trait_item_attrs(item: &syn::TraitItem) -> &[syn::Attribute] {
    match item {
        syn::TraitItem::Const(i) => &i.attrs,
        syn::TraitItem::Fn(i) => &i.attrs,
        syn::TraitItem::Type(i) => &i.attrs,
        syn::TraitItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn trait_item_name(item: &syn::TraitItem) -> Option<String> {
    match item {
        syn::TraitItem::Const(i) => Some(i.ident.to_string()),
        syn::TraitItem::Fn(i) => Some(i.sig.ident.to_string()),
        syn::TraitItem::Type(i) => Some(i.ident.to_string()),
        _ => None,
    }
}

pub(crate) fn foreign_item_attrs(item: &syn::ForeignItem) -> &[syn::Attribute] {
    match item {
        syn::ForeignItem::Fn(i) => &i.attrs,
        syn::ForeignItem::Static(i) => &i.attrs,
        syn::ForeignItem::Type(i) => &i.attrs,
        syn::ForeignItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

fn foreign_item_name(item: &syn::ForeignItem) -> Option<String> {
    match item {
        syn::ForeignItem::Fn(i) => Some(i.sig.ident.to_string()),
        syn::ForeignItem::Static(i) => Some(i.ident.to_string()),
        syn::ForeignItem::Type(i) => Some(i.ident.to_string()),
        _ => None,
    }
}

/// Shared by the tests of other modules, which need a [`Gates`] but nothing
/// from a manifest.
#[cfg(test)]
pub(crate) mod tests_support {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::metadata::Package;

    /// A package declaring no features and no dependencies.
    pub(crate) fn bare_package() -> Package {
        Package {
            name: "fixture".to_string(),
            manifest_path: PathBuf::from("/ws/Cargo.toml"),
            targets: Vec::new(),
            dependencies: Vec::new(),
            features: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::metadata::{Dependency, Package};

    fn package(features: &[(&str, &[&str])], optional: &[&str]) -> Package {
        Package {
            name: "fixture".to_string(),
            manifest_path: PathBuf::from("/ws/Cargo.toml"),
            targets: Vec::new(),
            dependencies: optional
                .iter()
                .map(|name| Dependency {
                    name: (*name).to_string(),
                    rename: None,
                    kind: None,
                    optional: true,
                    target: None,
                })
                .collect(),
            features: features
                .iter()
                .map(|(name, enables)| {
                    (
                        (*name).to_string(),
                        enables.iter().map(|e| (*e).to_string()).collect(),
                    )
                })
                .collect(),
        }
    }

    /// The gate on the first item of `source`, under the configured matrix.
    fn configured(gates: &Gates<'_>, source: &str) -> Truth {
        let file: syn::File = syn::parse_str(source).expect("fixture must parse");
        eval_attrs(attrs_of(&file.items[0]), &gates.configured())
    }

    /// The same gate, under every build that could ever exist.
    fn maximal(gates: &Gates<'_>, source: &str) -> Truth {
        let file: syn::File = syn::parse_str(source).expect("fixture must parse");
        eval_attrs(attrs_of(&file.items[0]), &gates.maximal())
    }

    /// The property the whole phase rests on: with no `[cfg]` section, every
    /// gate that could ever hold is still followed, exactly as before.
    #[test]
    fn the_default_matrix_follows_every_gate_that_can_hold() {
        let matrix = Matrix::default();
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        for source in [
            "#[cfg(test)]\nmod tests {}\n",
            "#[cfg(unix)]\nmod platform {}\n",
            "#[cfg(windows)]\nmod platform {}\n",
            "#[cfg(target_os = \"redox\")]\nmod platform {}\n",
            "#[cfg(feature = \"std\")]\nmod gated {}\n",
            "#[cfg(not(feature = \"std\"))]\nmod gated {}\n",
            "#[cfg(any(unix, feature = \"std\"))]\nmod gated {}\n",
            "#[cfg(all(unix, not(target_os = \"macos\")))]\nmod gated {}\n",
            // Unevaluable in every direction.
            "#[cfg(accessible(::std::mem))]\nmod gated {}\n",
            "#[cfg(docsrs)]\nmod gated {}\n",
            "#[cfg_attr(feature = \"nope\", cfg(feature = \"nope\"))]\nmod gated {}\n",
        ] {
            let file: syn::File = syn::parse_str(source).unwrap();
            assert!(
                gates.compiled(attrs_of(&file.items[0])),
                "the default matrix must follow `{source}`"
            );
        }
    }

    /// A feature the manifest does not declare can be turned on by nobody, on
    /// no platform, in no checkout.
    #[test]
    fn a_gate_on_an_undeclared_feature_can_never_hold() {
        let matrix = Matrix::default();
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        assert_eq!(
            maximal(&gates, "#[cfg(feature = \"nope\")]\nmod gone {}\n"),
            Truth::Never
        );
        // ...but its negation holds everywhere, and `any` is satisfied by the
        // other arm.
        assert_eq!(
            maximal(&gates, "#[cfg(not(feature = \"nope\"))]\nmod kept {}\n"),
            Truth::Always
        );
        assert_eq!(
            maximal(
                &gates,
                "#[cfg(any(feature = \"nope\", feature = \"std\"))]\nmod kept {}\n"
            ),
            Truth::Either
        );
    }

    /// Whether the first item of `source` is confined to a test build, asked
    /// the way its own kind requires: a `fn` is the one [`Site::FreeFn`].
    fn confined(gates: &Gates<'_>, source: &str) -> bool {
        let file: syn::File = syn::parse_str(source).expect("fixture must parse");
        let item = &file.items[0];
        let site = match item {
            syn::Item::Fn(_) => Site::FreeFn,
            _ => Site::Other,
        };
        gates.test_only(attrs_of(item), site)
    }

    /// The claim this answer exists for, and the one `#[cfg(test)]` never
    /// covered: `#[test]` confines a function to a test build on its own.
    /// Verified against rustc — a bare `#[test] fn` naming a crate that does
    /// not exist compiles as a library and fails under `--test`.
    #[test]
    fn a_test_attribute_confines_a_function_to_a_test_build() {
        let matrix = Matrix::default();
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        for source in [
            "#[test]\nfn t() {}\n",
            "#[bench]\nfn b() {}\n",
            // Beside gates that hold in a non-test build, which is winnow's
            // shape: the test attribute is what confines it, and the gates
            // only say which builds compile the tests.
            "#[test]\n#[cfg(feature = \"std\")]\n#[cfg(unix)]\nfn t() {}\n",
            // With `#[should_panic]`, which is not itself a confinement but
            // accompanies one.
            "#[test]\n#[should_panic]\nfn t() {}\n",
            // Already-confined code says the same thing twice.
            "#[test]\n#[cfg(test)]\nfn t() {}\n",
        ] {
            assert!(confined(&gates, source), "`{source}` is test-only");
        }
    }

    /// The boundary. Everything here confines nothing — though the attribute
    /// macros among them are not read as leaving their item attributed either:
    /// [`unexpandable_macro`] answers that separately, and [`crate::deps`]
    /// moves such an item's mentions to the opaque context.
    #[test]
    fn nothing_but_a_bare_test_or_bench_attribute_confines_an_item() {
        let matrix = Matrix::default();
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        for source in [
            // `#[should_panic]` alone leaves the function in the library
            // build: rustc resolves its body under `--crate-type=lib`.
            "#[should_panic]\nfn p() {}\n",
            // A proc-macro test attribute is a macro Deadwood cannot expand.
            "#[tokio::test]\nasync fn t() {}\n",
            "#[rstest]\nfn t() {}\n",
            // The built-in attribute's own path spelling, which rustc *does*
            // honour, and which is indistinguishable from the line above
            // before expansion. A missed finding, deliberately.
            "#[core::prelude::v1::test]\nfn t() {}\n",
            // Not the attribute at all, however much it looks like it.
            "#[test_case(1)]\nfn t() {}\n",
            // The attribute with arguments is not the built-in one either.
            "#[test(flavor = \"multi_thread\")]\nfn t() {}\n",
            // `cfg_attr` is an indirection this module does not follow, here
            // as everywhere else in it.
            "#[cfg_attr(unix, test)]\nfn t() {}\n",
            // rustc rejects `#[test]` on anything but a `fn`, so these are
            // `Site::Other` and only their gates count.
            "#[test]\nmod tests {}\n",
            "#[test]\ninclude!(\"generated.rs\");\n",
            "#[test]\nstruct S;\n",
            "#[test]\nimpl S {}\n",
        ] {
            assert!(!confined(&gates, source), "`{source}` confines nothing");
        }
    }

    /// Whether the first item of `source` is owned by an attribute macro.
    fn owned_by_a_macro(source: &str) -> bool {
        let file: syn::File = syn::parse_str(source).expect("fixture must parse");
        unexpandable_macro(attrs_of(&file.items[0]))
    }

    /// What is an attribute macro: on stable rustc, every attribute that is
    /// not built in, not a tool's, and not a derive helper can be nothing
    /// else. Each spelling here is one an item could really carry.
    #[test]
    fn an_attribute_that_is_not_built_in_a_tools_or_a_helper_is_a_macro() {
        for source in [
            // The multi-segment path, in the spelling issue #49 filed.
            "#[tokio::test]\nasync fn t() {}\n",
            // The path the built-in attribute expands to, which nothing
            // distinguishes from a proc macro before expansion.
            "#[core::prelude::v1::test]\nfn t() {}\n",
            // A single-segment attribute brought into scope by `use`: no
            // built-in has this name and there is no derive to belong to.
            "#[rstest]\nfn t() {}\n",
            // Arguments change nothing about what the path names.
            "#[serial_test::serial(alpha)]\nfn t() {}\n",
            // On items a `#[test]` could never confine: ownership has no site.
            "#[async_trait::async_trait]\nimpl S {}\n",
            "#[wasm_bindgen]\nstruct S;\n",
        ] {
            assert!(owned_by_a_macro(source), "`{source}` is macro input");
        }
    }

    /// What is not: the attribute kinds that rewrite nothing. Sweeping any of
    /// these into opacity would make placeable mentions unplaceable — the
    /// corpus's own `src/` trees carry `target_feature`, `deprecated` and
    /// `rustfmt::skip`, and nothing else that is not a gate.
    #[test]
    fn built_in_tool_and_helper_attributes_are_not_macros() {
        for source in [
            // Built-in attributes, including the ones the corpus carries.
            "#[inline]\nfn f() {}\n",
            "#[deprecated]\nfn f() {}\n",
            "#[target_feature(enable = \"avx2\")]\nunsafe fn f() {}\n",
            "#[must_use]\nfn f() {}\n",
            "#[test]\nfn t() {}\n",
            "#[unsafe(no_mangle)]\nfn f() {}\n",
            // Tool namespaces.
            "#[rustfmt::skip]\nfn f() {}\n",
            "#[clippy::cognitive_complexity = \"30\"]\nfn f() {}\n",
            "#[diagnostic::on_unimplemented(message = \"no\")]\ntrait T {}\n",
            // A derive helper: only legal beside its derive, and inert.
            "#[derive(Serialize)]\n#[serde(rename_all = \"kebab-case\")]\nstruct S;\n",
            // `cfg_attr` is an indirection this module does not follow — and
            // needs not: in every build whose predicate fails, the item is
            // compiled exactly as written.
            "#[cfg_attr(test, tokio::test)]\nasync fn t() {}\n",
            // No attributes at all.
            "fn f() {}\n",
        ] {
            assert!(!owned_by_a_macro(source), "`{source}` rewrites nothing");
        }
    }

    /// The helper exemption is scoped to the item carrying the derive: the
    /// same unknown single-segment attribute with no derive beside it can only
    /// be an attribute macro.
    #[test]
    fn an_unknown_attribute_is_only_a_helper_beside_a_derive() {
        assert!(owned_by_a_macro("#[serde]\nstruct S;\n"));
        assert!(!owned_by_a_macro(
            "#[derive(Deserialize)]\n#[serde]\nstruct S;\n"
        ));
    }

    /// An item compiled by no build at all is dead by construction, not test
    /// code — which `#[cfg(test)]` has always answered this way and a test
    /// attribute must not be able to override. Attributing its mentions to the
    /// tests would place them in a build that does not exist either.
    #[test]
    fn a_test_function_behind_an_impossible_gate_is_not_test_only() {
        let matrix = Matrix::default();
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        assert!(!confined(
            &gates,
            "#[test]\n#[cfg(feature = \"nope\")]\nfn t() {}\n"
        ));
        assert!(!confined(
            &gates,
            "#[cfg(all(test, feature = \"nope\"))]\nfn t() {}\n"
        ));
    }

    /// A test attribute is not a configuration predicate, so it is not a gate
    /// site, it makes nothing unsatisfiable, and — the load-bearing half —
    /// [`prune`] never removes the function. Deleting it would take its
    /// references out of every detector's view, which is a different phase's
    /// claim and not this one's.
    #[test]
    fn a_test_attribute_is_not_a_cfg_gate() {
        let matrix = Matrix::new(None, None, Some(false));
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        let source = "#[test]\nfn t() {}\n#[bench]\nfn b() {}\n#[cfg(test)]\nfn gated() {}\n";
        let mut file: syn::File = syn::parse_str(source).unwrap();
        assert!(
            gates.gate_sites(&file).len() == 1,
            "only `cfg(test)` is one"
        );
        assert!(gates.compiled(attrs_of(&file.items[0])));
        assert!(gates.compiled(attrs_of(&file.items[1])));

        prune(&gates, &mut file);
        let kept: Vec<String> = file.items.iter().filter_map(item_name).collect();
        assert_eq!(kept, vec!["t".to_string(), "b".to_string()]);
    }

    /// An optional dependency gets an implicit feature of the same name, so
    /// code gating on it is not gating on nothing.
    #[test]
    fn an_optional_dependencys_implicit_feature_counts_as_declared() {
        let matrix = Matrix::default();
        let manifest = package(&[], &["serde"]);
        let gates = Gates::new(&matrix, &manifest);
        assert_eq!(
            maximal(&gates, "#[cfg(feature = \"serde\")]\nmod wire {}\n"),
            Truth::Either
        );
    }

    #[test]
    fn narrowing_features_rules_out_the_ones_left_off() {
        let matrix = Matrix::new(Some(vec!["default".to_string()]), None, None);
        // `default` pulls in `std`, so a gate on `std` holds; `extra` does not.
        let manifest = package(&[("default", &["std"]), ("std", &[]), ("extra", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        assert_eq!(
            configured(&gates, "#[cfg(feature = \"std\")]\nmod on {}\n"),
            Truth::Always
        );
        assert_eq!(
            configured(&gates, "#[cfg(feature = \"extra\")]\nmod off {}\n"),
            Truth::Never
        );
        let file: syn::File = syn::parse_str("#[cfg(feature = \"extra\")]\nmod off {}\n").unwrap();
        assert!(
            !gates.compiled(attrs_of(&file.items[0])),
            "a feature the matrix leaves off is not part of the analyzed build"
        );
    }

    /// An impossible gate is reported, never hidden: excluding the code behind
    /// it would move what every other detector sees.
    #[test]
    fn an_impossible_gate_is_still_followed() {
        let matrix = Matrix::new(Some(vec!["std".to_string()]), None, None);
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);
        let file: syn::File = syn::parse_str("#[cfg(feature = \"nope\")]\nmod gone {}\n").unwrap();
        assert!(gates.compiled(attrs_of(&file.items[0])));
    }

    #[test]
    fn narrowing_targets_decides_platform_gates_and_families() {
        let matrix = Matrix::new(None, Some(vec!["linux".to_string()]), None);
        let manifest = package(&[], &[]);
        let gates = Gates::new(&matrix, &manifest);

        let cases = [
            ("#[cfg(unix)]\nmod p {}\n", Truth::Always),
            ("#[cfg(windows)]\nmod p {}\n", Truth::Never),
            ("#[cfg(target_os = \"linux\")]\nmod p {}\n", Truth::Always),
            ("#[cfg(target_os = \"macos\")]\nmod p {}\n", Truth::Never),
            (
                "#[cfg(target_family = \"unix\")]\nmod p {}\n",
                Truth::Always,
            ),
            // Never modelled, so never narrowed.
            (
                "#[cfg(target_arch = \"x86_64\")]\nmod p {}\n",
                Truth::Either,
            ),
        ];
        for (source, expected) in cases {
            assert_eq!(configured(&gates, source), expected, "{source}");
        }
    }

    /// A target name we do not recognize must not be sorted into a family by
    /// guesswork.
    #[test]
    fn an_unknown_target_os_leaves_families_undecided() {
        let matrix = Matrix::new(None, Some(vec!["someos".to_string()]), None);
        let manifest = package(&[], &[]);
        let gates = Gates::new(&matrix, &manifest);
        assert_eq!(
            configured(&gates, "#[cfg(unix)]\nmod p {}\n"),
            Truth::Either
        );
        assert_eq!(
            configured(&gates, "#[cfg(windows)]\nmod p {}\n"),
            Truth::Never,
            "an OS that is not `windows` is not the windows family"
        );
    }

    #[test]
    fn test_gated_code_is_in_the_matrix_by_default_and_droppable_by_choice() {
        let manifest = package(&[], &[]);

        let default = Matrix::default();
        let gates = Gates::new(&default, &manifest);
        assert_eq!(
            configured(&gates, "#[cfg(test)]\nmod tests {}\n"),
            Truth::Either
        );

        let without = Matrix::new(None, None, Some(false));
        let gates = Gates::new(&without, &manifest);
        assert_eq!(
            configured(&gates, "#[cfg(test)]\nmod tests {}\n"),
            Truth::Never
        );
        assert_eq!(
            configured(&gates, "#[cfg(not(test))]\nmod real {}\n"),
            Truth::Always
        );
    }

    #[test]
    fn pruning_removes_only_what_the_matrix_rules_out() {
        let matrix = Matrix::new(None, None, Some(false));
        let manifest = package(&[], &[]);
        let gates = Gates::new(&matrix, &manifest);

        let mut file: syn::File = syn::parse_str(
            "pub fn kept() {}\n\
             #[cfg(test)]\nmod tests { fn inner() {} }\n\
             #[cfg(unix)]\npub fn platform() {}\n\
             struct Held;\nimpl Held { #[cfg(test)] fn probe(&self) {} fn real(&self) {} }\n",
        )
        .unwrap();
        prune(&gates, &mut file);

        let names: Vec<Option<String>> = file.items.iter().map(item_name).collect();
        assert_eq!(
            names,
            vec![
                Some("kept".to_string()),
                Some("platform".to_string()),
                Some("Held".to_string()),
                None,
            ],
            "the `#[cfg(test)]` module goes, the platform gate stays"
        );
        let syn::Item::Impl(block) = file.items.last().unwrap() else {
            panic!("expected the impl block");
        };
        assert_eq!(block.items.len(), 1, "the test-only method goes too");
    }

    #[test]
    fn gate_sites_report_the_outermost_impossible_gate_only() {
        let matrix = Matrix::default();
        let manifest = package(&[("std", &[])], &[]);
        let gates = Gates::new(&matrix, &manifest);

        let file: syn::File = syn::parse_str(
            "#[cfg(feature = \"gone\")]\nmod dead { #[cfg(feature = \"alsogone\")] fn deeper() {} }\n\
             #[cfg(feature = \"std\")]\nfn alive() {}\n",
        )
        .unwrap();
        let sites = gates.gate_sites(&file);

        assert_eq!(sites.len(), 2, "the nested gate is not visited");
        assert_eq!(sites[0].name.as_deref(), Some("dead"));
        assert_eq!(sites[0].gate, "feature = \"gone\"");
        let Verdict::Impossible { undeclared } = &sites[0].verdict else {
            panic!("the gate names no declared feature");
        };
        assert_eq!(undeclared, &vec!["gone".to_string()]);
        assert!(matches!(sites[1].verdict, Verdict::CanHold));
    }

    #[test]
    fn a_platform_dependency_table_is_judged_against_the_target_matrix() {
        let manifest = package(&[], &[]);

        let default = Matrix::default();
        let gates = Gates::new(&default, &manifest);
        assert_eq!(
            gates.target_expression("cfg(windows)"),
            TargetVerdict::Possible
        );
        assert_eq!(
            gates.target_expression("cfg(any())"),
            TargetVerdict::NeverBuilt,
            "the version-pinning idiom is compiled by no target on any matrix"
        );

        let linux = Matrix::new(None, Some(vec!["linux".to_string()]), None);
        let gates = Gates::new(&linux, &manifest);
        assert_eq!(
            gates.target_expression("cfg(unix)"),
            TargetVerdict::Possible
        );
        assert_eq!(
            gates.target_expression("cfg(windows)"),
            TargetVerdict::RuledOutByMatrix
        );
        assert_eq!(
            gates.target_expression("x86_64-pc-windows-msvc"),
            TargetVerdict::Possible,
            "a bare triple is not modelled, so its entries are judged as usual"
        );
    }

    #[test]
    fn an_optional_dependency_is_judgeable_only_when_a_feature_can_enable_it() {
        let manifest = package(
            &[
                ("serde", &["dep:serde"]),
                ("default", &["extras"]),
                ("extras", &["dep:rare"]),
            ],
            &["serde", "rare"],
        );

        let default = Matrix::default();
        let gates = Gates::new(&default, &manifest);
        assert!(gates.optional_dependency_possible("serde"));
        assert!(gates.optional_dependency_possible("rare"));

        let narrowed = Matrix::new(Some(vec!["default".to_string()]), None, None);
        let gates = Gates::new(&narrowed, &manifest);
        assert!(
            gates.optional_dependency_possible("rare"),
            "`default` enables `extras`, which enables `rare`"
        );
        assert!(
            !gates.optional_dependency_possible("serde"),
            "nothing in the configured closure turns `serde` on"
        );
    }

    /// `dep?/feature` forwards a feature only when the dependency is already
    /// on; it is not what turns it on.
    #[test]
    fn a_weak_feature_reference_does_not_enable_its_dependency() {
        let manifest = package(&[("std", &["serde?/std"])], &["serde"]);
        let matrix = Matrix::new(Some(vec!["std".to_string()]), None, None);
        let gates = Gates::new(&matrix, &manifest);
        assert!(!gates.optional_dependency_possible("serde"));
    }
}
