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
//! - `test`, which the matrix admits by default (see `docs/SCOPE.md` for why
//!   the quiet default was chosen).
//! - `target_os = "..."`, `target_family = "..."`, `unix`, `windows`.
//! - `not`, `all`, `any` over any of those, at any nesting depth.
//!
//! Everything else — `target_arch`, `target_env`, `debug_assertions`,
//! `miri`, a bare `cfg` a build script sets — is [`Truth::Either`]. The matrix
//! has no axis for them, so no configuration is ruled out by one, and the
//! answer is honest rather than guessed.

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

fn attrs_of(item: &syn::Item) -> &[syn::Attribute] {
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

fn impl_item_attrs(item: &syn::ImplItem) -> &[syn::Attribute] {
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

fn trait_item_attrs(item: &syn::TraitItem) -> &[syn::Attribute] {
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

fn foreign_item_attrs(item: &syn::ForeignItem) -> &[syn::Attribute] {
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
