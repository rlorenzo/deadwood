//! Path-aware usage resolution.
//!
//! This module answers one question for every `pub` item in a workspace: does
//! any path anywhere actually *refer to it*? Unlike a name census, a mention
//! of the identifier is not enough — the path it appears in has to resolve to
//! that item's definition.
//!
//! # How it works
//!
//! 1. **Symbol table** — each target (lib, bin, test, example, bench, build
//!    script) is a crate. For every crate we build its module tree from
//!    [`ParsedFile::module`] plus inline `mod` blocks, and record the items
//!    each module defines, the `use` aliases it binds, and the glob imports
//!    it pulls in.
//! 2. **Glob expansion** — `use path::*` is resolved to the module it names
//!    when that module is in the workspace, so names it brings into scope
//!    still resolve. A glob we cannot follow (an external crate, most often)
//!    makes the importing module *opaque*.
//! 3. **Reference walk** — every `syn::Path` in every file is resolved from
//!    the module it is written in, following `crate::`, `self::`, `super::`,
//!    workspace crate names, `use` aliases, and re-export chains. Each
//!    definition a path names is marked used.
//! 4. **Lexical scopes** — a single-segment path that a local binding,
//!    parameter, or generic parameter covers names that binding and not an
//!    item, so it resolves to nothing. See [Lexical scopes](#lexical-scopes).
//! 5. **Reachability** — each use is recorded against the definition the
//!    naming path is written *inside*, and only the definitions reachable
//!    from a root are alive. See [Reachability](#reachability).
//!
//! # Conservatism
//!
//! Deadwood prefers a missed finding to a wrong one, so anything it cannot
//! resolve is treated as a use of *every* item with that name in the
//! workspace:
//!
//! - identifiers inside macro invocations and attribute arguments, since we
//!   do not expand macros;
//! - names spelled inside an attribute's *string* arguments, where derives
//!   routinely hide real paths (`#[serde(with = "crate::codec")]`); if such a
//!   name is a module, everything in it counts as used, because that is the
//!   form those attributes take;
//! - names that are not in scope in a module that has an unfollowable glob
//!   import, since the glob may well be where they come from;
//! - paths that run through an alias or module we cannot pin down.
//!
//! Doc comments are the one string we do not scan: they are prose, and
//! mentioning an item in one should not keep it alive.
//!
//! A path that resolves cleanly to nothing in the workspace (`std`, an
//! external crate, or a local name matching no item) marks nothing.
//!
//! # Lexical scopes
//!
//! `let helper = 5; helper` names the local, not the module's `pub fn
//! helper`, so the walk tracks bindings and resolves nothing for a path one
//! of them covers. Suppressing a path is the one thing in this module that
//! can *invent* a finding rather than lose one, so every rule below is the
//! narrow side of a choice:
//!
//! - **Namespaces are separate.** Rust resolves values and types apart, so a
//!   `let` binding shadows only *expression* paths and a generic parameter
//!   shadows only *type* paths. A binding set applied by name alone would let
//!   `let Foo = 1;` silence the `: Foo` on the next line and report a live
//!   type as dead.
//! - **Only a bare name is ever shadowed.** `helper::thing` names a module
//!   even where `helper` is bound, and so does `::helper`.
//! - **Order is respected.** A `let` initializer is resolved before its
//!   pattern binds (`let x = x();` still names the item `x`), and a `let ...
//!   else` block is resolved where the binding does not exist yet.
//! - **Scopes pop.** Blocks, `match` arms, `if let`/`while let` branches,
//!   `for` patterns and closure bodies each end their bindings, and an item
//!   nested in a function body starts from an empty scope, because it cannot
//!   see that function's locals or generics.
//! - **A pattern is not automatically a binding.** `let Foo(x) = y;` and
//!   `Foo { field: x }` name a struct or variant; a *bare* name in pattern
//!   position is a unit-struct, unit-variant or `const` pattern whenever one
//!   is in scope and a fresh binding otherwise. Telling those apart is the
//!   sharpest edge here, so the symbol table decides: a name that could name
//!   such an item is marked used and binds nothing.
//!
//! Where the position of a path cannot be established the path is resolved
//! as before, which is how a construct this module does not model — a pattern
//! in macro input, say — keeps costing findings rather than precision.
//!
//! # Reachability
//!
//! "Something names this" is not the same as "something alive names this".
//! `pub fn orphan() { helper(); }` keeps `helper` referenced for exactly as
//! long as `orphan` exists, and a pair of mutually recursive functions nothing
//! reaches keeps *itself* referenced forever. So every use is recorded against
//! the definition the naming path is written inside — a [`Referrer`] — and a
//! definition is alive only when a walk from the root set gets to it.
//!
//! Reporting an item that *is* resolved and referenced, on the strength of a
//! claim about its referrer, is the one thing in this module whose failure
//! mode is a false positive by construction rather than by bug. Two rules keep
//! that in hand:
//!
//! - **A missing referrer is a root, never an absence.** Anything opaque —
//!   macro input, attribute arguments, an unfollowable glob, an alias we
//!   cannot pin down — is already a use of every item of that name, and it
//!   becomes a *root* rather than an edge. A mention we admit we cannot read
//!   must not be turned into evidence that something is dead. So is a use
//!   written where there is no definition to attribute it to: at module level,
//!   in an `impl` block for a type outside the workspace, inside an item
//!   nested in a function body.
//! - **A root is not exempt from being reported.** The report subtracts
//!   nothing: an item nothing names is a finding exactly as it was before
//!   reachability existed. That is what lets a library's whole public surface
//!   be a root — consumers we cannot see call it, so a path written inside one
//!   is no evidence — without silencing a single finding Deadwood used to
//!   make. See [`SymbolTable::unused_definitions`] and
//!   [`SymbolTable::reachable`].
//!
//! # Known limitations
//!
//! - Purely syntactic: method calls (`x.foo()`), trait dispatch, and
//!   associated items are not resolved. Only free-standing item definitions
//!   are reported, so this costs findings, never precision.
//! - Reachability follows references, not containment: an item inside a module
//!   nothing names is judged on the paths that name *it*. A module can be
//!   reached without being named — through a glob, through a `pub use`, from
//!   generated code — so treating "unnamed module" as "everything in it is
//!   dead" would be a claim about code we have not seen.
//! - An `impl` block hangs off its self type and, where we can resolve it, the
//!   trait it implements. That is right for an inherent impl and for a trait
//!   impl on a workspace type, and it is a root — costing findings, not
//!   precision — for everything else: a foreign self type, a blanket
//!   `impl<T>`, a tuple, a reference, an array.
//! - Lexical scopes are tracked syntactically, so what a *macro* binds is
//!   invisible: an identifier in macro input already counts as a use of every
//!   item with that name, and a local a macro expands to shadows nothing.
//!   Loop labels, lifetimes and `self` need no tracking — none of them can
//!   name an item.
//! - Only the code in the analyzed build reaches here: items a `cfg` the
//!   configured matrix rules out are removed before the symbol table is built
//!   ([`crate::cfg`]), so they neither define nor use anything. With the
//!   default matrix that is every item there is — every feature combination
//!   and every target is analyzed, and `#[cfg(test)]` code counts as a use, so
//!   an item only tests reach is not reported. `[cfg] test = false` is how a
//!   project asks the other question.
//! - Edition 2015 crate-relative `use` paths are supported by falling back to
//!   the crate root; other 2015-only path forms may resolve to nothing (which
//!   only ever hides findings).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;

use crate::config::PublicApi;
use crate::modtree::ParsedFile;

/// How deep a chain of `use` aliases is followed before giving up and falling
/// back to the conservative rule. Real code never comes close.
const MAX_ALIAS_DEPTH: usize = 8;

/// One compilation unit: a crate root plus every file reachable from it.
pub(crate) struct CrateUnit {
    /// Names other crates can use to refer to this one in paths. Empty for
    /// targets nothing can name (bins, tests, examples, benches).
    pub names: Vec<String>,
    /// Whether the whole target is test code: a `test`, `bench` or `example`
    /// target is compiled by `cargo test`/`cargo bench` and by nothing a
    /// consumer runs, so *everything* in one is test code — not only its
    /// `#[test]` functions. See [`EntryPoint`].
    pub test_code: bool,
    pub files: Vec<ParsedFile>,
}

/// Whether something outside the workspace's own paths reaches a definition,
/// and — the part that needs two answers — whether a build with no tests in it
/// does.
///
/// Splitting this out of a single `bool` is what lets the reachability walk run
/// twice over one edge set ([`RootSet`]): once from every entry point, which is
/// the build Deadwood analyzes, and once from the entry points that survive
/// `[cfg] test = false`, which is the build a consumer of the crate gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryPoint {
    /// Not an entry point: the definition lives or dies by the walk.
    None,
    /// Only a build that compiles the tests reaches it — `#[test]` and
    /// `#[bench]` functions, plus every entry point written in code that is
    /// test code by where it sits: a `test`, `bench` or `example` target, or a
    /// file only `#[cfg(test)] mod` declarations reach
    /// ([`ParsedFile::test_only`]).
    Test,
    /// Every build reaches it: `fn main`, the linker and compiler exports, and
    /// the `dead_code` opt-outs.
    NonTest,
}

impl EntryPoint {
    /// An entry point written in code that is, or is not, test code.
    fn of_context(test_context: bool) -> EntryPoint {
        if test_context {
            EntryPoint::Test
        } else {
            EntryPoint::NonTest
        }
    }
}

/// Which entry points seed a reachability walk.
///
/// One walk with a parameter rather than two walks written out: the difference
/// between the two answers *is* the test-only claim, so they must not be able
/// to drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootSet {
    /// Every entry point, which is the build being analyzed and the answer
    /// every finding before this one was made against.
    Full,
    /// The same set with the test entry points removed. Everything else is
    /// unchanged — a library's public surface, `[public-api]`, and everything
    /// opaque are roots here too.
    WithoutTests,
}

impl RootSet {
    /// Whether an entry point of this kind seeds this walk.
    fn admits(self, entry: EntryPoint) -> bool {
        match entry {
            EntryPoint::None => false,
            EntryPoint::NonTest => true,
            EntryPoint::Test => self == RootSet::Full,
        }
    }
}

/// What a definition is: decides how a path may continue through it and how
/// a finding about it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefKind {
    Fn,
    Struct,
    Enum,
    Trait,
    TypeAlias,
    Const,
    Static,
    Union,
    /// A `mod` declaration or an `extern crate` alias; paths continue into
    /// the module it opens.
    Mod,
    /// A `use` alias that is not part of the crate's surface (`use`,
    /// `pub(crate) use`, or any `use` inside a function body).
    Import,
    /// A module-level `pub use` alias: part of the surface, and reportable in
    /// its own right when nothing goes through it.
    Reexport,
}

impl DefKind {
    /// How the kind is named in a report.
    pub(crate) fn label(self) -> &'static str {
        match self {
            DefKind::Fn => "fn",
            DefKind::Struct => "struct",
            DefKind::Enum => "enum",
            DefKind::Trait => "trait",
            DefKind::TypeAlias => "type alias",
            DefKind::Const => "const",
            DefKind::Static => "static",
            DefKind::Union => "union",
            DefKind::Mod => "mod",
            DefKind::Import | DefKind::Reexport => "re-export",
        }
    }

    /// Whether a finding about this definition is about a re-export rather
    /// than a definition of its own.
    pub(crate) fn is_reexport(self) -> bool {
        self == DefKind::Reexport
    }

    fn is_alias(self) -> bool {
        matches!(self, DefKind::Import | DefKind::Reexport)
    }
}

/// A path as written, reduced to the parts that matter for resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RefPath {
    /// `::foo::Bar`: the first segment names an external crate, never an item
    /// in scope.
    absolute: bool,
    segments: Vec<String>,
}

impl RefPath {
    /// A bare one-segment path, as a pattern or an identifier spells it.
    fn single(name: &str) -> Self {
        RefPath {
            absolute: false,
            segments: vec![name.to_string()],
        }
    }

    fn from_syn(path: &syn::Path) -> Self {
        RefPath {
            absolute: path.leading_colon.is_some(),
            segments: path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect(),
        }
    }
}

struct Def {
    name: String,
    kind: DefKind,
    file: PathBuf,
    line: usize,
    /// The module the definition is written in.
    module: usize,
    /// Whether an unreferenced occurrence is worth reporting: `pub`, not
    /// `fn main`, and not opted out with an attribute.
    reportable: bool,
    /// Whether the definition is fully `pub`, which — inside a library, under
    /// `pub` modules all the way to the crate root — is what makes it surface
    /// that code we cannot see may call. [`Def::reportable`] is not a stand-in
    /// for this: it also excludes `fn main` and the attribute escape hatches,
    /// which are roots for entirely different reasons.
    is_pub: bool,
    /// Whether something outside the source calls this — an entry point, a
    /// linker export, or an explicit opt-out — and whether a build without
    /// tests is one of the things that does. See [`entry_point_attr`].
    entry_point: EntryPoint,
    /// For [`DefKind::Mod`]: the module this name opens into.
    child: Option<usize>,
    /// For alias kinds: the path being imported, to be resolved from
    /// [`Def::module`].
    target: Option<RefPath>,
}

struct Module {
    krate: usize,
    parent: Option<usize>,
    path: Vec<String>,
    /// Name to definitions. Rust's namespaces are merged here: a `mod` and a
    /// `fn` sharing a name both resolve, which can only hide findings.
    items: HashMap<String, Vec<usize>>,
    /// `use prefix::*` prefixes written in this module, before resolution,
    /// each with whether the glob is a `pub use` — which re-exports the names
    /// it pulls in rather than merely importing them.
    globs: Vec<(RefPath, bool)>,
    /// Modules whose names this module pulls in through a resolved glob.
    glob_sources: Vec<usize>,
    /// The subset of those a `pub use` glob pulls in, so their items can be
    /// named from here by anyone who can name this module. See
    /// [`SymbolTable::externally_reachable_modules`].
    pub_glob_sources: Vec<usize>,
    /// A glob import that could not be followed into the workspace, so a name
    /// missing from `items` might still refer to a workspace item.
    opaque: bool,
    /// Whether the `mod` declaration that opens this module is `pub`. The
    /// crate root has no declaration and is always considered public.
    declared_pub: bool,
}

/// Where a name lookup in a module landed.
enum Step {
    /// The name is defined here (possibly several times).
    Defs(Vec<usize>),
    /// The name is not defined here, and this module's scope is fully known.
    Absent,
    /// The name is not defined here, but the scope has holes we cannot see
    /// into, so it may still be a workspace item.
    Unknown,
}

/// Where the definitions named by a path segment lead.
enum Target {
    Module(usize),
    /// A concrete item: any further segments are associated items.
    Item,
    Unknown,
}

/// The result of walking a whole path.
enum Outcome {
    /// Every segment resolved, ending at this module.
    Module(usize),
    /// Ended at a concrete item, or at its associated items.
    Item,
    /// Nothing in this workspace: an external crate, a local binding, a
    /// generic parameter, a prelude item.
    Foreign,
    /// A workspace item might be hidden behind a glob or an alias we could
    /// not follow. Carries the index of the first segment we could not
    /// account for.
    Opaque(usize),
}

/// Every definition in the workspace, the modules holding them, and which
/// ones a resolved path reaches.
pub(crate) struct SymbolTable {
    defs: Vec<Def>,
    modules: Vec<Module>,
    /// Root module of each crate, indexed by crate.
    roots: Vec<usize>,
    /// Crate name to that crate's root module.
    externs: HashMap<String, usize>,
    /// Crate names claimed by more than one crate. Resolving through one
    /// would be a guess, so paths that start with one fall back to the
    /// conservative rule instead.
    ambiguous_externs: HashSet<String>,
    /// Every definition sharing a name, for the conservative fallback.
    by_name: HashMap<String, Vec<usize>>,
    /// Module lookup by crate and module path.
    by_path: HashMap<(usize, Vec<String>), usize>,
    /// Whether each crate is a library, i.e. whether anything outside the
    /// workspace could name its public items at all.
    is_library: Vec<bool>,
    /// The name each crate is spelled by in paths, for the `public-api`
    /// allowlist. `None` for targets nothing can name (bins, tests, examples,
    /// benches), which have no external consumers to declare.
    crate_names: Vec<Option<String>>,
    /// Definitions by the site they are written at, so the reference walk can
    /// find the definition a path is written *inside*. Keyed by module rather
    /// than file because one file pulled into two crates defines its items
    /// once per crate.
    defs_at: HashMap<(usize, usize), Vec<usize>>,
    /// Every definition some resolved path names, whoever named it. This is
    /// phase 1's answer and still half the report.
    used: HashSet<usize>,
    /// Referrer to the definitions named from inside it: the edges
    /// reachability walks.
    edges: HashMap<usize, Vec<usize>>,
    /// Definitions named from a context that is alive whatever else is —
    /// see [`Referrer::Root`].
    rooted: HashSet<usize>,
}

/// A reportable definition that nothing live reaches.
pub(crate) struct UnusedDef {
    pub name: String,
    pub kind: DefKind,
    pub file: PathBuf,
    pub line: usize,
    /// The module the definition is written in, as
    /// [`SymbolTable::module_path`] spells it.
    pub module: String,
    /// Whether paths do name this definition and every one of them is written
    /// inside something nothing reaches. `false` is the older and stronger
    /// claim: no path names it at all.
    pub only_from_unreached: bool,
}

/// A reportable definition that only test code reaches.
///
/// It carries no `only_from_unreached` twin: this claim is made *because* the
/// item is referenced and reached, so there is only one kind of evidence for
/// it. See [`SymbolTable::test_only_definitions`].
pub(crate) struct TestOnlyDef {
    pub name: String,
    pub kind: DefKind,
    pub file: PathBuf,
    pub line: usize,
    /// The module the definition is written in, as
    /// [`SymbolTable::module_path`] spells it.
    pub module: String,
}

/// What a use is attributed to: the definition the naming path is written
/// inside.
///
/// This is the whole of reachability. A use recorded against a definition
/// holds only while that definition is itself reached, and a use recorded
/// against [`Referrer::Root`] holds unconditionally.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Referrer {
    /// The use is not inside any definition we can name, or came through a
    /// channel we cannot see into. It counts on its own.
    ///
    /// Everything opaque lands here deliberately: an identifier in macro
    /// input, a name in an attribute's string argument, and a name an
    /// unfollowable glob may have brought into scope are all already read as
    /// uses of *every* item with that name ("Conservatism", above). Reading
    /// them as ordinary edges instead would let an opaque mention whose only
    /// visible referrer is dead cascade into a false positive, which is the
    /// one failure this whole check has to avoid.
    Root,
    /// Written inside these definitions; the use holds if any one of them is
    /// reached. More than one only happens for an `impl` block, which hangs
    /// off its self type and, when we can see it, the trait it implements.
    Defs(Vec<usize>),
}

impl SymbolTable {
    /// Index every definition, alias, and glob import in `crates`.
    pub(crate) fn build(crates: &[CrateUnit]) -> Self {
        let mut table = SymbolTable {
            defs: Vec::new(),
            modules: Vec::new(),
            roots: Vec::new(),
            externs: HashMap::new(),
            ambiguous_externs: HashSet::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            is_library: crates.iter().map(|unit| !unit.names.is_empty()).collect(),
            crate_names: crates
                .iter()
                .map(|unit| unit.names.first().cloned())
                .collect(),
            defs_at: HashMap::new(),
            used: HashSet::new(),
            edges: HashMap::new(),
            rooted: HashSet::new(),
        };

        for krate in 0..crates.len() {
            let root = table.new_module(krate, None, Vec::new());
            table.roots.push(root);
        }
        // A target's own library name is authoritative; the package name and
        // any dependency aliases are fallbacks, used only for names no
        // library target claims. Within a tier, a name claimed by two crates
        // is ambiguous: picking either would risk marking the wrong crate's
        // items used and reporting the right one's as dead.
        let mut authoritative: HashMap<&str, HashSet<usize>> = HashMap::new();
        let mut fallback: HashMap<&str, HashSet<usize>> = HashMap::new();
        for (krate, unit) in crates.iter().enumerate() {
            let mut names = unit.names.iter();
            if let Some(name) = names.next() {
                authoritative
                    .entry(name.as_str())
                    .or_default()
                    .insert(table.roots[krate]);
            }
            for name in names {
                fallback
                    .entry(name.as_str())
                    .or_default()
                    .insert(table.roots[krate]);
            }
        }
        for (name, roots) in authoritative.iter().chain(
            fallback
                .iter()
                .filter(|(name, _)| !authoritative.contains_key(*name)),
        ) {
            let mut roots = roots.iter();
            match (roots.next(), roots.next()) {
                (Some(&root), None) => {
                    table.externs.insert((*name).to_string(), root);
                }
                _ => {
                    table.ambiguous_externs.insert((*name).to_string());
                }
            }
        }

        for (krate, unit) in crates.iter().enumerate() {
            for file in &unit.files {
                let Some(ast) = &file.ast else { continue };
                let module = table.module_for(krate, &file.module);
                // Where the file sits decides what an entry point in it means:
                // a `fn main` in an example, or in a file only `#[cfg(test)]`
                // declarations reach, is only ever run by a test build.
                let test_context = unit.test_code || file.test_only;
                table.collect_items(&ast.items, module, file, true, test_context);
            }
        }

        table.resolve_globs();
        table
    }

    /// Resolve every path in `crates`, marking the definitions they name.
    pub(crate) fn record_references(&mut self, crates: &[CrateUnit]) {
        for (krate, unit) in crates.iter().enumerate() {
            for file in &unit.files {
                let Some(ast) = &file.ast else { continue };
                let Some(&module) = self.by_path.get(&(krate, file.module.clone())) else {
                    continue;
                };
                let mut walker = RefWalker {
                    table: self,
                    krate,
                    module,
                    module_path: file.module.clone(),
                    impl_self: None,
                    pos: PathPos::Other,
                    value_scopes: Vec::new(),
                    type_scopes: Vec::new(),
                    enclosing: Referrer::Root,
                };
                for item in &ast.items {
                    walker.visit_item(item);
                }
            }
        }

        // One edge per referrer and target, recorded once rather than checked
        // on every insert. A function calling the same helper twenty times
        // pushes twenty identical edges, and the walk needs one — across the
        // crates in a local registry that is 65% of the slots. Deduping here
        // rather than in `mark_def_used` keeps the hot path a push.
        for targets in self.edges.values_mut() {
            targets.sort_unstable();
            targets.dedup();
        }
    }

    /// Reportable definitions nothing live reaches, ordered by source
    /// location.
    ///
    /// A definition survives on two conditions, and failing either is a
    /// finding:
    ///
    /// 1. **Something names it.** This is phase 1's question, unchanged, and
    ///    it is what keeps every finding Deadwood made before reachability
    ///    existed. A root is not exempt from it: a library's `pub fn` that
    ///    nothing in the workspace calls is reported exactly as it always was.
    /// 2. **Something live names it.** A path written inside a definition
    ///    nothing reaches is not evidence of anything, so an item whose every
    ///    referrer is itself dead is dead — which is what makes a dead
    ///    subsystem, and a dead cycle, come out in one run instead of one
    ///    layer per run.
    ///
    /// A dead cycle reports every member rather than the group once. Each is
    /// separately deletable and separately located, and the alternative needs
    /// a name for the group — which the baseline keys on, and which would move
    /// whenever a member joined or left it
    /// ([#16](https://github.com/rlorenzo/deadwood/issues/16) is already open
    /// on that key being weaker than it looks). Falling out of the
    /// per-definition rule is also what makes the answer identical run to run.
    ///
    /// Definitions are deduplicated by location: a file pulled into several
    /// crates (via `#[path]`) defines the same item once per crate, and it
    /// counts as used if *any* of those crates uses it. `public_api` is
    /// consulted before that deduplication, so a file shared between a listed
    /// crate and an unlisted one is judged by each crate's own listing rather
    /// than by whichever copy happened to be indexed first.
    pub(crate) fn unused_definitions(&self, public_api: &PublicApi) -> Vec<UnusedDef> {
        let site = |def: &Def| (def.file.clone(), def.line, def.name.clone());
        let surface = self.externally_reachable_modules();
        let used: HashSet<_> = self.used.iter().map(|&id| site(&self.defs[id])).collect();
        let reached: HashSet<_> = self
            .reachable(public_api, &surface, RootSet::Full)
            .iter()
            .map(|&id| site(&self.defs[id]))
            .collect();

        let mut seen = HashSet::new();
        let mut out: Vec<UnusedDef> = self
            .defs
            .iter()
            .filter(|def| def.reportable && self.is_worth_reporting(def, &surface))
            .filter(|def| !self.is_declared_api(def, public_api))
            .filter(|def| {
                let site = site(def);
                !(used.contains(&site) && reached.contains(&site)) && seen.insert(site)
            })
            .map(|def| UnusedDef {
                name: def.name.clone(),
                kind: def.kind,
                file: def.file.clone(),
                line: def.line,
                module: self.module_path(def),
                only_from_unreached: used.contains(&site(def)),
            })
            .collect();
        out.sort_by(|a, b| (&a.file, a.line, &a.name).cmp(&(&b.file, b.line, &b.name)));
        out
    }

    /// Reportable definitions the analyzed build reaches and a build without
    /// its tests does not.
    ///
    /// The claim is the difference between two walks over one edge set, and it
    /// is narrower than "this is dead" on purpose. Three conditions, and the
    /// first two are exactly what makes an item *quiet* in
    /// [`SymbolTable::unused_definitions`]:
    ///
    /// 1. **Something names it**, and 2. **something live names it** — so a
    ///    test-only finding and an unused-pub finding can never be made about
    ///    the same definition. An item nothing names is dead, which is the
    ///    stronger claim and the one already reported; saying "only tests reach
    ///    this" about it as well would be two findings for one deletion.
    /// 3. **Nothing reaches it once the test entry points are gone**
    ///    ([`RootSet::WithoutTests`]).
    ///
    /// What this cannot say is as important as what it can, and one rule
    /// answers for most of it: a library's public surface is a root in *both*
    /// walks — every `pub` item under `pub` modules from the crate root, and
    /// everything a `pub use` glob re-exports from the crate root or one of
    /// those modules ([`SymbolTable::externally_reachable_modules`]) — so a
    /// `pub fn` a consumer could name is never test-only however plainly only
    /// the tests call it here. We cannot see the consumers, and claiming
    /// otherwise is the false positive this whole check is shaped to avoid.
    /// And everything opaque is a root in both walks too ([`Referrer::Root`]):
    /// one mention in macro input — an `assert_eq!` naming the item is the
    /// common one — keeps an item out of this kind entirely. Every one of
    /// those costs findings, and none of them invents one.
    pub(crate) fn test_only_definitions(&self, public_api: &PublicApi) -> Vec<TestOnlyDef> {
        let site = |def: &Def| (def.file.clone(), def.line, def.name.clone());
        let surface = self.externally_reachable_modules();
        let used: HashSet<_> = self.used.iter().map(|&id| site(&self.defs[id])).collect();
        let sites = |roots| -> HashSet<_> {
            self.reachable(public_api, &surface, roots)
                .iter()
                .map(|&id| site(&self.defs[id]))
                .collect()
        };
        let reached = sites(RootSet::Full);
        let without_tests = sites(RootSet::WithoutTests);

        let mut seen = HashSet::new();
        let mut out: Vec<TestOnlyDef> = self
            .defs
            .iter()
            .filter(|def| def.reportable && self.is_worth_reporting(def, &surface))
            // No `is_declared_api` filter here, and no surface filter either:
            // both are roots in *both* walks, so neither can reach the
            // conditions below. `unused_definitions` needs the first because an
            // item nothing names is reported however rooted it is; this claim is
            // only ever made about items something does name. The surface filter
            // was phase 10's second copy of the rule
            // ([#25](https://github.com/rlorenzo/deadwood/issues/25)), and it is
            // gone because the root set now answers for it.
            .filter(|def| {
                let site = site(def);
                used.contains(&site)
                    && reached.contains(&site)
                    && !without_tests.contains(&site)
                    && seen.insert(site)
            })
            .map(|def| TestOnlyDef {
                name: def.name.clone(),
                kind: def.kind,
                file: def.file.clone(),
                line: def.line,
                module: self.module_path(def),
            })
            .collect();
        out.sort_by(|a, b| (&a.file, a.line, &a.name).cmp(&(&b.file, b.line, &b.name)));
        out
    }

    /// Every definition reached from a root by following the edges the
    /// reference walk recorded.
    ///
    /// The root set is the whole of the risk in this check: every omission is
    /// a live item reported dead. It is
    ///
    /// - **everything opaque** — [`SymbolTable::rooted`], filled by every use
    ///   whose referrer we could not name and by every conservative
    ///   by-name fallback (see [`Referrer::Root`]);
    /// - **every entry point `roots` admits** — `fn main`, `#[test]`,
    ///   `#[bench]`, the linker and compiler exports, and the `dead_code`
    ///   opt-outs ([`entry_point_attr`]). [`RootSet::WithoutTests`] drops the
    ///   test ones, and that difference is the whole of
    ///   [`SymbolTable::test_only_definitions`];
    /// - **a library's public surface** — every `pub` definition in a module
    ///   [`SymbolTable::externally_reachable_modules`] covers: `pub` modules
    ///   all the way to the crate root of a crate something outside the
    ///   workspace can name, and the modules a `pub use` glob exports from
    ///   there. Consumers we cannot see call these, so a use written inside
    ///   one is not evidence that anything is dead. That the surface *itself*
    ///   is still reported when nothing in the workspace names it is condition
    ///   1 in [`SymbolTable::unused_definitions`], and is why rooting it costs
    ///   no finding Deadwood used to make;
    /// - **whatever `[public-api]` declares**, which is the same claim made
    ///   deliberately rather than inferred, and reaches items in private
    ///   modules and in binaries that the surface rule cannot.
    ///
    /// Two things deliberately outside it. A definition that is not `pub` is
    /// an ordinary node: rustc's `dead_code` lint already reaches private
    /// items this way, so rooting them would only stop the cascade at the
    /// first private helper — while `pub fn orphan()` calling `fn glue()`
    /// calling `pub fn helper()` is exactly the chain rustc cannot see and
    /// this check exists for. And containment is not reference: an item in a
    /// dead module is judged on the paths that name *it*, because a module
    /// being unnamed says nothing about who reaches inside it.
    fn reachable(
        &self,
        public_api: &PublicApi,
        surface: &HashSet<usize>,
        roots: RootSet,
    ) -> HashSet<usize> {
        let mut stack: Vec<usize> = self.rooted.iter().copied().collect();
        stack.extend(
            (0..self.defs.len()).filter(|&id| self.is_root(id, public_api, surface, roots)),
        );
        let mut seen: HashSet<usize> = stack.iter().copied().collect();
        while let Some(id) = stack.pop() {
            for &next in self.edges.get(&id).into_iter().flatten() {
                if seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        seen
    }

    /// Whether this definition is alive before any edge is followed. See
    /// [`SymbolTable::reachable`] for why each clause is here.
    ///
    /// `roots` changes exactly one clause: whether the test entry points are in
    /// it. A library's public surface stays a root in both walks, because a
    /// consumer we cannot see reaches it in a build with no tests at all —
    /// which is why nothing on a library's surface is ever test-only.
    ///
    /// `surface` is [`SymbolTable::externally_reachable_modules`], computed
    /// once by the caller and shared by both walks so the two answers cannot
    /// differ in anything but the entry points.
    fn is_root(
        &self,
        id: usize,
        public_api: &PublicApi,
        surface: &HashSet<usize>,
        roots: RootSet,
    ) -> bool {
        let def = &self.defs[id];
        roots.admits(def.entry_point)
            || (def.is_pub && surface.contains(&def.module))
            || self.is_declared_api(def, public_api)
    }

    /// Whether an unreferenced definition of this kind, in this position, is
    /// a finding a reader can act on.
    ///
    /// A `pub use` exists only to expose a name outward, so one that is
    /// reachable from a library's crate root is doing its job even when
    /// nothing inside the workspace goes through it — that is the whole
    /// public-API idiom (`pub use inner::Thing;` in `lib.rs`), and reporting
    /// it would bury the real findings. A re-export that outside code cannot
    /// even reach, because a module on the way is private, has no such
    /// excuse. Definitions are judged on their own `pub`ness, as before: an
    /// over-permissive `pub` is exactly what that check is for.
    ///
    /// "Reachable from the crate root" is
    /// [`SymbolTable::externally_reachable_modules`], the same rule the root
    /// set uses, so a `pub use` inside a module a `pub use inner::*;` exports
    /// is doing its job for exactly the reason an item beside it is nameable.
    /// Asking the narrower question here and the wider one there is the drift
    /// this phase existed to remove.
    fn is_worth_reporting(&self, def: &Def, surface: &HashSet<usize>) -> bool {
        !def.kind.is_reexport() || !surface.contains(&def.module)
    }

    /// Whether the project has declared this item part of a crate's public
    /// API, so that having no consumer inside the workspace is expected.
    ///
    /// The item is offered to the allowlist as `crate::module::path::Item`,
    /// with the crate name the crate answers to in paths — the same spelling
    /// a `use` would need.
    fn is_declared_api(&self, def: &Def, public_api: &PublicApi) -> bool {
        let module = &self.modules[def.module];
        let krate = self.crate_names[module.krate].as_deref();
        let mut path = String::new();
        for segment in krate
            .into_iter()
            .chain(module.path.iter().map(String::as_str))
        {
            path.push_str(segment);
            path.push_str("::");
        }
        path.push_str(&def.name);
        public_api.covers(krate, &path)
    }

    /// The module a definition is written in, as a path a reader can navigate
    /// by: `crate`, `crate::alpha`, `crate::lexical::math::small`.
    ///
    /// Crate-*relative* on purpose, and the crate name is deliberately not the
    /// head segment the way [`SymbolTable::is_declared_api`] spells one. One
    /// file can belong to several crates — a `#[path]` share, or a target root
    /// read once per target — and a finding about it is deduplicated across
    /// them by site, so keying anything on the crate name would make the answer
    /// depend on which copy the deduplication happened to keep. The module path
    /// itself is a property of the source: where the item is written inside its
    /// file's own module tree.
    ///
    /// `crate` rather than an empty string for the root, because this value
    /// ends up in [`crate::Finding::module`] and from there in a baseline
    /// entry, where "the crate root" and "no module recorded" have to be
    /// different values — the first is a module a key can be compared on and
    /// the second is an entry that predates the field. `crate` is not a legal
    /// module name, so the two can never be confused.
    fn module_path(&self, def: &Def) -> String {
        let module = &self.modules[def.module];
        let mut path = String::from("crate");
        for segment in &module.path {
            path.push_str("::");
            path.push_str(segment);
        }
        path
    }

    /// Every module whose items code outside the workspace could name: a
    /// library's public surface, and the only rule in this module that answers
    /// that question.
    ///
    /// [`SymbolTable::is_pub_to_the_crate_root`] answers most of it — `pub`
    /// modules all the way to a library's crate root — and misses one form:
    /// `mod inner; pub use inner::*;` puts `inner`'s items on the surface under
    /// the re-exporting module's path without `inner` being `pub` at all.
    /// `winnow`'s `combinator::iterator` is that shape, and reporting it
    /// because the crate's own tests are the only callers *here* would be a
    /// finding about documented public API.
    ///
    /// A named `pub use` needs no such treatment: it is a definition of its
    /// own, it is a root when its module is on the surface, and reaching it
    /// reaches what it names. A glob binds no name and so records no edge,
    /// which is exactly the hole this fills.
    ///
    /// The closure follows two edges, and needs both. A `pub use` glob reaches
    /// the module it names, and a surface module reaches its own `pub`
    /// children — `pub use inner::*;` re-exports `inner::nested` as well as
    /// `inner`'s functions, so `facade::nested::item` is nameable and stopping
    /// at `inner` would leave the same false positive one level down.
    ///
    /// Three callers, one answer, and that is the point of the shape. The root
    /// set ([`SymbolTable::is_root`]) roots what a consumer could call, the
    /// re-export filter ([`SymbolTable::is_worth_reporting`]) asks whether a
    /// `pub use` is doing its job by existing, and
    /// [`SymbolTable::test_only_definitions`] gets its answer from the root set
    /// rather than filtering a second time. Phase 10 consulted a second copy of
    /// this rule from the last of those alone, where it could only remove a
    /// finding; folding it into the root set is
    /// [#25](https://github.com/rlorenzo/deadwood/issues/25), and it is what
    /// keeps the two from drifting.
    ///
    /// A glob whose target is *outside* the workspace reaches nothing here — it
    /// is unresolvable, so it makes its module opaque instead, and opaque is
    /// already a root in every walk. A cross-crate glob within the workspace
    /// (`pub use other_member::*;`) adds nothing either: the only modules of
    /// another member a path can name are `pub` from that member's own crate
    /// root, so this rule already covers them.
    fn externally_reachable_modules(&self) -> HashSet<usize> {
        // Only `pub` children: a private `mod` inside a glob-exported module is
        // no more nameable from outside than a private `mod` anywhere else.
        let mut pub_children: Vec<Vec<usize>> = vec![Vec::new(); self.modules.len()];
        for (id, module) in self.modules.iter().enumerate() {
            if let Some(parent) = module.parent
                && module.declared_pub
            {
                pub_children[parent].push(id);
            }
        }

        let mut seen: HashSet<usize> = (0..self.modules.len())
            .filter(|&module| self.is_pub_to_the_crate_root(module))
            .collect();
        let mut stack: Vec<usize> = seen.iter().copied().collect();
        while let Some(module) = stack.pop() {
            let reached = self.modules[module]
                .pub_glob_sources
                .iter()
                .chain(&pub_children[module]);
            for &source in reached {
                if seen.insert(source) {
                    stack.push(source);
                }
            }
        }
        seen
    }

    /// Whether this module belongs to a library and every `mod` on the way in
    /// from its crate root is `pub`.
    ///
    /// One of the two edges [`SymbolTable::externally_reachable_modules`] is
    /// built from, and its only caller: the surface question is asked of that
    /// set, never of this, because a module a `pub use` glob exports is on the
    /// surface without any `mod` on the way to it being `pub` at all.
    fn is_pub_to_the_crate_root(&self, module: usize) -> bool {
        let mut current = module;
        loop {
            let module = &self.modules[current];
            if !module.declared_pub {
                return false;
            }
            match module.parent {
                Some(parent) => current = parent,
                None => return self.is_library[module.krate],
            }
        }
    }

    // -- table construction ------------------------------------------------

    fn new_module(&mut self, krate: usize, parent: Option<usize>, path: Vec<String>) -> usize {
        let id = self.modules.len();
        self.modules.push(Module {
            krate,
            parent,
            path: path.clone(),
            items: HashMap::new(),
            globs: Vec::new(),
            glob_sources: Vec::new(),
            pub_glob_sources: Vec::new(),
            opaque: false,
            declared_pub: parent.is_none(),
        });
        self.by_path.insert((krate, path), id);
        id
    }

    /// The module at `path` in `krate`, creating it and its ancestors if the
    /// declaring file has not been visited yet.
    fn module_for(&mut self, krate: usize, path: &[String]) -> usize {
        let mut current = self.roots[krate];
        let mut built = Vec::new();
        for segment in path {
            built.push(segment.clone());
            current = match self.by_path.get(&(krate, built.clone())) {
                Some(&id) => id,
                None => self.new_module(krate, Some(current), built.clone()),
            };
        }
        current
    }

    fn add_def(&mut self, def: Def) -> usize {
        let id = self.defs.len();
        self.by_name.entry(def.name.clone()).or_default().push(id);
        self.modules[def.module]
            .items
            .entry(def.name.clone())
            .or_default()
            .push(id);
        self.defs_at
            .entry((def.module, def.line))
            .or_default()
            .push(id);
        self.defs.push(def);
        id
    }

    /// The definitions named `name` written at a site, for attributing the
    /// paths inside one to it.
    ///
    /// The name is not part of the key, because a key holding one would have
    /// to be built — and so allocated — at every lookup. A module and a line
    /// almost always hold a single definition, so filtering the handful there
    /// is cheaper than hashing a `String` to find them.
    ///
    /// Several matches only happens where two `use` leaves share a line, and
    /// taking all of them is the forgiving direction: the use holds if any one
    /// of them is reached.
    fn defs_at(&self, module: usize, line: usize, name: &str) -> Vec<usize> {
        self.defs_at
            .get(&(module, line))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&id| self.defs[id].name == name)
            .collect()
    }

    /// Record the module-level items of `items` into `module`.
    ///
    /// `top_level` is false inside a function body, where a `pub use` binds a
    /// name locally but re-exports nothing. `test_context` is true where the
    /// code itself is test code, which is what an entry point written here
    /// means ([`EntryPoint`]).
    fn collect_items(
        &mut self,
        items: &[syn::Item],
        module: usize,
        file: &ParsedFile,
        top_level: bool,
        test_context: bool,
    ) {
        for item in items {
            match item {
                syn::Item::Mod(m) => {
                    let krate = self.modules[module].krate;
                    let mut path = self.modules[module].path.clone();
                    path.push(m.ident.to_string());
                    let child = self.module_for(krate, &path);
                    self.modules[child].declared_pub = matches!(m.vis, syn::Visibility::Public(_));
                    self.add_def(Def {
                        name: m.ident.to_string(),
                        kind: DefKind::Mod,
                        file: file.path.clone(),
                        line: m.ident.span().start().line,
                        module,
                        reportable: false,
                        // A `mod` declaration is a leaf in the edge graph —
                        // paths written inside the module belong to the items
                        // holding them, not to the declaration — so whether it
                        // is reached decides nothing, and neither flag needs
                        // to claim anything.
                        is_pub: false,
                        entry_point: EntryPoint::None,
                        child: Some(child),
                        target: None,
                    });
                    if let Some((_, inner)) = &m.content {
                        self.collect_items(inner, child, file, true, test_context);
                    }
                }
                syn::Item::Use(u) => self.add_use(u, module, file, top_level, test_context),
                syn::Item::ExternCrate(e) => {
                    // `extern crate foo as bar;` binds `bar` to a crate root.
                    let name = e
                        .rename
                        .as_ref()
                        .map_or_else(|| e.ident.to_string(), |(_, alias)| alias.to_string());
                    if let Some(&root) = self.externs.get(&e.ident.to_string()) {
                        self.add_def(Def {
                            name,
                            kind: DefKind::Mod,
                            file: file.path.clone(),
                            line: e.ident.span().start().line,
                            module,
                            reportable: false,
                            is_pub: false,
                            entry_point: EntryPoint::None,
                            child: Some(root),
                            target: None,
                        });
                    }
                }
                other => {
                    if let Some((ident, kind, attrs, vis)) = describe(other) {
                        let name = ident.to_string();
                        let is_pub = matches!(vis, syn::Visibility::Public(_));
                        // `fn main` is the binary and build-script entry point
                        // and has been exempt from reporting since v0.1; being
                        // a root is the same fact stated for the cascade.
                        let is_main = kind == DefKind::Fn && name == "main";
                        let entry_point = if is_main {
                            EntryPoint::of_context(test_context)
                        } else {
                            entry_point_attr(attrs, test_context)
                        };
                        self.add_def(Def {
                            name,
                            kind,
                            file: file.path.clone(),
                            line: ident.span().start().line,
                            module,
                            reportable: is_pub && !is_main && !has_skip_attr(attrs),
                            is_pub,
                            entry_point,
                            child: None,
                            target: None,
                        });
                    }
                    // A `use` inside a function body still binds a name that
                    // paths in this file resolve through. Attributing it to
                    // the enclosing module widens its scope, which can only
                    // make resolution more forgiving.
                    let mut nested = NestedUses {
                        table: self,
                        module,
                        file,
                        test_context,
                    };
                    nested.visit_item(other);
                }
            }
        }
    }

    fn add_use(
        &mut self,
        item: &syn::ItemUse,
        module: usize,
        file: &ParsedFile,
        top_level: bool,
        test_context: bool,
    ) {
        let is_pub = matches!(item.vis, syn::Visibility::Public(_));
        let absolute = item.leading_colon.is_some();
        let mut leaves = Vec::new();
        flatten_use(&item.tree, &mut Vec::new(), &mut leaves);

        for leaf in leaves {
            let path = RefPath {
                absolute,
                segments: leaf.segments,
            };
            if leaf.glob {
                // A `pub use` inside a function body re-exports nothing, which
                // is the same reason `top_level` decides the kind below.
                self.modules[module].globs.push((path, is_pub && top_level));
                continue;
            }
            // `use x as _;` imports a trait's methods without binding a name;
            // there is nothing to reference and nothing to report.
            let Some(name) = leaf.alias else { continue };
            let kind = if is_pub && top_level {
                DefKind::Reexport
            } else {
                DefKind::Import
            };
            self.add_def(Def {
                name,
                kind,
                file: file.path.clone(),
                line: leaf.line,
                module,
                reportable: kind == DefKind::Reexport && !has_skip_attr(&item.attrs),
                // A `pub use` on a library's surface is reachable from
                // outside, so what it re-exports is too; a plain `use` is
                // reachable only from whatever goes through it, which is what
                // lets a dead import stop keeping its target alive.
                is_pub: kind == DefKind::Reexport,
                entry_point: entry_point_attr(&item.attrs, test_context),
                child: None,
                target: Some(path),
            });
        }
    }

    /// Point every glob import at the module it pulls names from.
    ///
    /// Globs can import from modules that are themselves filled by globs, so
    /// this repeats until nothing new resolves; whatever is left over refers
    /// outside the workspace and makes its module opaque.
    fn resolve_globs(&mut self) {
        let mut pending: Vec<(usize, RefPath, bool)> = self
            .modules
            .iter()
            .enumerate()
            .flat_map(|(id, m)| {
                m.globs
                    .iter()
                    .cloned()
                    .map(move |(glob, exported)| (id, glob, exported))
            })
            .collect();

        loop {
            let before = pending.len();
            let mut still_pending = Vec::new();
            for (module, glob, exported) in pending {
                match self.walk_path(module, &glob, true, 0, &mut Vec::new()) {
                    Outcome::Module(target) => {
                        self.modules[module].glob_sources.push(target);
                        if exported {
                            self.modules[module].pub_glob_sources.push(target);
                        }
                    }
                    _ => still_pending.push((module, glob, exported)),
                }
            }
            if still_pending.len() == before {
                for (module, ..) in still_pending {
                    self.modules[module].opaque = true;
                }
                return;
            }
            pending = still_pending;
        }
    }

    // -- resolution --------------------------------------------------------

    /// Look `name` up in `module`, following resolved glob imports.
    fn lookup(&self, module: usize, name: &str) -> Step {
        self.lookup_in(module, name, &mut HashSet::new())
    }

    fn lookup_in(&self, module: usize, name: &str, visited: &mut HashSet<usize>) -> Step {
        if !visited.insert(module) {
            // Already accounted for on another branch of a glob cycle.
            return Step::Absent;
        }
        if let Some(defs) = self.modules[module].items.get(name) {
            return Step::Defs(defs.clone());
        }
        let mut unknown = self.modules[module].opaque;
        for &source in &self.modules[module].glob_sources {
            match self.lookup_in(source, name, visited) {
                Step::Defs(defs) => return Step::Defs(defs),
                Step::Unknown => unknown = true,
                Step::Absent => {}
            }
        }
        if unknown { Step::Unknown } else { Step::Absent }
    }

    /// Whether a bare name written in pattern position could be naming an
    /// item rather than binding a fresh one.
    ///
    /// `let Foo = x;` is a *use* when `Foo` is a unit struct, a unit variant
    /// or a `const`, and a binding otherwise — syntax alone cannot tell them
    /// apart, so the symbol table answers. Only those three kinds can appear
    /// as a bare path pattern; a `fn`, `mod`, `trait`, `enum` or `static` of
    /// that name cannot, which is what makes `let helper = 5;` a binding even
    /// though `helper` names something.
    ///
    /// Uncertainty answers yes, and yes costs only a missed finding: reading
    /// a `const` pattern as a binding would suppress the genuine uses that
    /// follow it in the same scope and report a live item dead.
    fn pattern_may_name_item(&self, module: usize, name: &str) -> bool {
        match self.lookup(module, name) {
            // Nothing of that name in the workspace: whatever the pattern
            // means, suppressing it can hide no finding.
            Step::Absent => false,
            Step::Unknown => true,
            Step::Defs(defs) => defs.iter().any(|&id| {
                matches!(
                    self.defs[id].kind,
                    // A `use` alias can lead to any of the three, and a type
                    // alias can name a unit struct.
                    DefKind::Struct
                        | DefKind::Const
                        | DefKind::TypeAlias
                        | DefKind::Import
                        | DefKind::Reexport
                )
            }),
        }
    }

    /// Where a path segment's definitions lead: into a module, or to an item.
    fn step_into(&self, defs: &[usize], depth: usize) -> Target {
        let mut module = None;
        for &id in defs {
            let def = &self.defs[id];
            let candidate = if def.kind == DefKind::Mod {
                def.child
            } else if def.kind.is_alias() {
                let Some(target) = &def.target else { continue };
                match self.walk_path(def.module, target, true, depth + 1, &mut Vec::new()) {
                    Outcome::Module(id) => Some(id),
                    // An alias to a concrete item or to something outside the
                    // workspace ends the path here.
                    Outcome::Item | Outcome::Foreign => None,
                    Outcome::Opaque(_) => return Target::Unknown,
                }
            } else {
                None
            };
            match (candidate, module) {
                (Some(next), None) => module = Some(next),
                // Two modules behind one name: we cannot tell which the path
                // continues into.
                (Some(next), Some(previous)) if next != previous => return Target::Unknown,
                _ => {}
            }
        }
        match module {
            Some(id) => Target::Module(id),
            None => Target::Item,
        }
    }

    /// Resolve `path` from `module`, collecting the definitions it names into
    /// `reached`, one group per segment.
    ///
    /// Grouping is what lets a caller ask which definitions the *last* segment
    /// named — the item an `impl` block belongs to — without re-walking the
    /// path; every other caller flattens it.
    ///
    /// `in_use` marks paths written in a `use` declaration, which may be
    /// crate-root-relative in edition 2015.
    fn walk_path(
        &self,
        module: usize,
        path: &RefPath,
        in_use: bool,
        depth: usize,
        reached: &mut Vec<Vec<usize>>,
    ) -> Outcome {
        if depth > MAX_ALIAS_DEPTH {
            return Outcome::Opaque(0);
        }
        let segments = &path.segments;
        if segments.is_empty() {
            return Outcome::Foreign;
        }

        let mut index = 0;
        let mut current = module;
        // Whether the next segment is still the head of the path, where crate
        // names and the `crate`/`self`/`super` qualifiers apply.
        let mut at_head = true;

        if path.absolute {
            if self.ambiguous_externs.contains(&segments[0]) {
                return Outcome::Opaque(0);
            }
            let Some(&root) = self.externs.get(&segments[0]) else {
                return Outcome::Foreign;
            };
            current = root;
            index = 1;
            at_head = false;
        } else {
            match segments[0].as_str() {
                "crate" => {
                    current = self.roots[self.modules[module].krate];
                    index = 1;
                    at_head = false;
                }
                "self" => {
                    index = 1;
                    at_head = false;
                }
                "super" => {
                    while index < segments.len() && segments[index] == "super" {
                        let Some(parent) = self.modules[current].parent else {
                            // Above the crate root: not resolvable, but also
                            // not something we can attribute to an item.
                            return Outcome::Foreign;
                        };
                        current = parent;
                        index += 1;
                    }
                    at_head = false;
                }
                // `Self::assoc` in an impl or trait: associated items only.
                "Self" => return Outcome::Foreign,
                _ => {}
            }
        }

        while index < segments.len() {
            let name = &segments[index];
            let mut step = self.lookup(current, name);
            if at_head && matches!(step, Step::Absent) {
                if self.ambiguous_externs.contains(name.as_str()) {
                    return Outcome::Opaque(index);
                }
                if let Some(&root) = self.externs.get(name.as_str()) {
                    // A workspace crate named at the head of the path.
                    current = root;
                    index += 1;
                    at_head = false;
                    continue;
                }
                if in_use {
                    // Edition 2015 `use` paths start at the crate root.
                    step = self.lookup(self.roots[self.modules[module].krate], name);
                }
            }
            at_head = false;

            match step {
                Step::Defs(defs) => {
                    reached.push(defs.clone());
                    index += 1;
                    match self.step_into(&defs, depth) {
                        Target::Module(next) => current = next,
                        Target::Item => return Outcome::Item,
                        Target::Unknown => {
                            return if index < segments.len() {
                                Outcome::Opaque(index)
                            } else {
                                Outcome::Item
                            };
                        }
                    }
                }
                Step::Absent => return Outcome::Foreign,
                Step::Unknown => return Outcome::Opaque(index),
            }
        }
        Outcome::Module(current)
    }

    // -- marking -----------------------------------------------------------

    /// Mark everything `path`, written inside `from` in `module`, refers to.
    fn mark_path_used(&mut self, from: &Referrer, module: usize, path: &RefPath, in_use: bool) {
        let mut reached = Vec::new();
        let outcome = self.walk_path(module, path, in_use, 0, &mut reached);
        for id in reached.into_iter().flatten() {
            self.mark_def_used(from, id);
        }
        if let Outcome::Opaque(index) = outcome {
            // The tail we could not account for names anything of that name,
            // anywhere, and there is no telling what: an unconditional claim.
            for index in index..path.segments.len() {
                let name = path.segments[index].clone();
                self.mark_name_used(&name);
            }
        }
    }

    /// Every definition the last segment of `path`, written in `module`,
    /// names — following alias chains to what they ultimately mean.
    ///
    /// `None` whenever the answer is not certain, including when the path
    /// leads outside the workspace: an `impl` block for a foreign or generic
    /// type has no definition here to hang off, and reading that as "hangs off
    /// nothing" would report everything it calls as dead.
    fn owner_defs(&self, module: usize, path: &RefPath) -> Option<Vec<usize>> {
        let mut reached = Vec::new();
        match self.walk_path(module, path, false, 0, &mut reached) {
            Outcome::Item | Outcome::Module(_) => {}
            Outcome::Opaque(_) | Outcome::Foreign => return None,
        }
        let last = reached.last()?;
        if last.is_empty() {
            return None;
        }
        last.iter().map(|&id| self.terminal_def_of(id, 0)).collect()
    }

    /// Whether the bare `name`, written in `module`, means exactly the item
    /// `path` names.
    ///
    /// Used to decide whether a qualified `impl crate::thing::Wrapper` can
    /// treat a bare `Wrapper` in its body as the self-reference it looks
    /// like. The bare name may equally be some *other* `Wrapper` brought in
    /// by a `use`, and suppressing that would hide a real use and report a
    /// live item as dead, so anything short of certainty answers `false`.
    fn bare_name_means(&self, module: usize, path: &RefPath, name: &str) -> bool {
        let Step::Defs(defs) = self.lookup(module, name) else {
            return false;
        };
        // More than one definition behind the name: a path spelled with it
        // reaches all of them, and only some are the impl's own type.
        let [only] = defs[..] else {
            return false;
        };
        match (
            self.terminal_def(module, path, 0),
            self.terminal_def_of(only, 0),
        ) {
            (Some(from_path), Some(from_name)) => from_path == from_name,
            _ => false,
        }
    }

    /// The definition a path ultimately names, following alias chains.
    /// Read-only twin of the marking walk; `None` whenever the answer is not
    /// certain.
    fn terminal_def(&self, module: usize, path: &RefPath, depth: usize) -> Option<usize> {
        if depth > MAX_ALIAS_DEPTH {
            return None;
        }
        let mut reached = Vec::new();
        match self.walk_path(module, path, false, 0, &mut reached) {
            Outcome::Item | Outcome::Module(_) => {}
            Outcome::Opaque(_) | Outcome::Foreign => return None,
        }
        self.terminal_def_of(*reached.last()?.last()?, depth)
    }

    /// The definition an alias ultimately names; the definition itself when
    /// it is not an alias.
    fn terminal_def_of(&self, id: usize, depth: usize) -> Option<usize> {
        if depth > MAX_ALIAS_DEPTH {
            return None;
        }
        match self.defs[id].target.clone() {
            Some(target) => self.terminal_def(self.defs[id].module, &target, depth + 1),
            None => Some(id),
        }
    }

    fn mark_def_used(&mut self, from: &Referrer, id: usize) {
        // The edge is recorded before the visited check, because a second
        // referrer of the same definition is new information even though the
        // definition is not: it is another way for it to stay alive.
        match from {
            Referrer::Root => {
                self.rooted.insert(id);
            }
            Referrer::Defs(defs) => {
                for &referrer in defs {
                    self.edges.entry(referrer).or_default().push(id);
                }
            }
        }
        if !self.used.insert(id) {
            // Already expanded; this also stops cycles of mutual re-exports.
            // The alias edge below is a property of the alias and not of who
            // reached it, so recording it once is recording it for good.
            return;
        }
        // Reaching an alias reaches whatever it re-exports — from the alias,
        // so that an import nothing goes through stops keeping its target
        // alive.
        if let Some(target) = self.defs[id].target.clone() {
            let module = self.defs[id].module;
            self.mark_path_used(&Referrer::Defs(vec![id]), module, &target, true);
        }
    }

    /// The conservative fallback: treat every definition with this name,
    /// anywhere in the workspace, as used — and as a root.
    ///
    /// Every caller is a channel we cannot see through: macro input, an
    /// attribute's arguments, a name an unfollowable glob may have brought
    /// into scope, a path through an alias we could not pin down. We do not
    /// know which item the mention meant, so we certainly do not know which
    /// item it was written inside; claiming an edge from the enclosing
    /// definition would be a claim we cannot support, and the direction it
    /// fails in is a live item reported dead.
    fn mark_name_used(&mut self, name: &str) {
        let Some(ids) = self.by_name.get(name) else {
            return;
        };
        for id in ids.clone() {
            self.mark_def_used(&Referrer::Root, id);
        }
    }

    /// Mark every identifier in an unexpanded token stream, at any nesting
    /// depth. Macro input can name anything, and we do not expand it.
    fn mark_tokens_used(&mut self, tokens: &TokenStream) {
        for tree in tokens.clone() {
            match tree {
                TokenTree::Ident(ident) => self.mark_name_used(&ident.to_string()),
                TokenTree::Group(group) => self.mark_tokens_used(&group.stream()),
                _ => {}
            }
        }
    }

    /// Mark identifiers in an attribute's arguments, including ones spelled
    /// inside string literals: `#[serde(with = "crate::codec")]` and friends
    /// name real items in a form only the deriving macro understands.
    fn mark_attr_tokens_used(&mut self, tokens: &TokenStream) {
        for tree in tokens.clone() {
            match tree {
                TokenTree::Ident(ident) => self.mark_name_used(&ident.to_string()),
                TokenTree::Group(group) => self.mark_attr_tokens_used(&group.stream()),
                TokenTree::Literal(literal) => {
                    for word in literal
                        .to_string()
                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                        .filter(|word| !word.is_empty())
                    {
                        self.mark_name_in_attr_string_used(word);
                    }
                }
                TokenTree::Punct(_) => {}
            }
        }
    }

    /// Mark a name spelled inside an attribute's string argument.
    ///
    /// When the name is a module, everything in it is marked too: a string
    /// like `#[serde(with = "crate::codec")]` names the module, and the
    /// generated code then calls items inside it that appear nowhere else.
    fn mark_name_in_attr_string_used(&mut self, name: &str) {
        let Some(ids) = self.by_name.get(name) else {
            return;
        };
        let modules: Vec<usize> = ids.iter().filter_map(|&id| self.defs[id].child).collect();
        self.mark_name_used(name);
        for module in modules {
            let items: Vec<usize> = self.modules[module]
                .items
                .values()
                .flatten()
                .copied()
                .collect();
            for id in items {
                self.mark_def_used(&Referrer::Root, id);
            }
        }
    }

    fn mark_path_names_used(&mut self, path: &syn::Path) {
        for segment in &path.segments {
            self.mark_name_used(&segment.ident.to_string());
        }
    }
}

/// Which namespace a path is written in, and so which bindings can shadow it.
///
/// [`Visit::visit_path`] sees a bare [`syn::Path`], so the namespace has to
/// arrive from the parent node. It is recorded here rather than resolved in
/// the parents because `visit_path` must stay the single place a path is
/// resolved: a dozen node kinds own a `syn::Path`, and each would otherwise
/// need its own copy of the `impl_self` check and the descent into generic
/// arguments. Only the parents that *establish* a namespace set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathPos {
    /// An expression path, shadowed by `let` bindings, parameters, and const
    /// generic parameters.
    Expr,
    /// A type path or trait bound, shadowed by generic type parameters.
    Type,
    /// Everywhere else — a pattern, a struct literal, a macro name, a `pub(in
    /// ...)` qualifier. Never shadowed, which is the resolution this module
    /// had before scopes existed.
    Other,
}

/// Walks a file's AST and resolves every path it contains.
struct RefWalker<'a> {
    table: &'a mut SymbolTable,
    krate: usize,
    module: usize,
    module_path: Vec<String>,
    /// Head segment of the self type of the `impl` block being walked, if it
    /// is a plain path. Inside its own `impl`, a type naming itself is just
    /// `Self` spelled out, not evidence that anything else uses it.
    impl_self: Option<String>,
    /// Namespace of the path currently being resolved.
    pos: PathPos,
    /// Value-namespace bindings in scope, innermost frame last.
    value_scopes: Vec<HashSet<String>>,
    /// Type-namespace bindings — generic parameters — innermost frame last.
    type_scopes: Vec<HashSet<String>>,
    /// The definition the paths being walked are written inside, which is what
    /// turns a flat set of uses into a graph.
    enclosing: Referrer,
}

impl RefWalker<'_> {
    /// Run `body` with one more binding frame in each namespace.
    fn in_scope(&mut self, body: impl FnOnce(&mut Self)) {
        self.value_scopes.push(HashSet::new());
        self.type_scopes.push(HashSet::new());
        body(self);
        self.value_scopes.pop();
        self.type_scopes.pop();
    }

    /// Run `body` resolving paths as `pos`, restoring the outer position
    /// after — a type inside an expression is still a type.
    fn with_pos(&mut self, pos: PathPos, body: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.pos, pos);
        body(self);
        self.pos = outer;
    }

    /// Run `body` attributing the uses inside it to `from`.
    fn within(&mut self, from: Referrer, body: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.enclosing, from);
        body(self);
        self.enclosing = outer;
    }

    /// The definition written at this site, as a referrer.
    ///
    /// [`Referrer::Root`] when there is none, which is how an item nested in a
    /// function body — never collected into the symbol table, so never
    /// reportable and never reachable — keeps what it names alive instead of
    /// silently condemning it.
    fn defined_at(&self, line: usize, name: &str) -> Referrer {
        let ids = self.table.defs_at(self.module, line, name);
        if ids.is_empty() {
            Referrer::Root
        } else {
            Referrer::Defs(ids)
        }
    }

    /// The definition an item is, as a referrer for the paths written inside
    /// it. Items with no definition of their own — an `impl` block, a `use`, a
    /// macro invocation at item level — are handled by the visitors below.
    fn item_referrer(&self, item: &syn::Item) -> Referrer {
        match describe(item) {
            Some((ident, ..)) => self.defined_at(ident.span().start().line, &ident.to_string()),
            None => Referrer::Root,
        }
    }

    /// Bind `name` in the innermost scope. With no scope open there is
    /// nothing to shadow — a binding outside every body would be a construct
    /// we do not model — so the binding is dropped rather than widened.
    fn bind_value(&mut self, name: String) {
        if let Some(frame) = self.value_scopes.last_mut() {
            frame.insert(name);
        }
    }

    fn bind_type(&mut self, name: String) {
        if let Some(frame) = self.type_scopes.last_mut() {
            frame.insert(name);
        }
    }

    /// Whether a binding covers this path, so that it names no item.
    fn shadowed(&self, path: &RefPath) -> bool {
        // Only a bare name can be a binding: `helper::thing` and `::helper`
        // both name a module however `helper` is bound here.
        if path.absolute || path.segments.len() != 1 {
            return false;
        }
        let frames = match self.pos {
            PathPos::Expr => &self.value_scopes,
            PathPos::Type => &self.type_scopes,
            PathPos::Other => return false,
        };
        frames.iter().any(|frame| frame.contains(&path.segments[0]))
    }

    /// Resolve the paths a pattern names and bind the names it introduces,
    /// into the innermost scope.
    fn bind_pat(&mut self, pat: &syn::Pat) {
        self.with_pos(PathPos::Other, |walker| walker.bind_pat_inner(pat));
    }

    fn bind_pat_inner(&mut self, pat: &syn::Pat) {
        match pat {
            syn::Pat::Ident(node) => {
                for attr in &node.attrs {
                    self.visit_attribute(attr);
                }
                let name = node.ident.to_string();
                // `ref x`, `mut x` and `x @ sub` are binding syntax outright;
                // a plain name has to be asked about.
                let is_binding = node.by_ref.is_some()
                    || node.mutability.is_some()
                    || node.subpat.is_some()
                    || !self.table.pattern_may_name_item(self.module, &name);
                if is_binding {
                    self.bind_value(name);
                } else {
                    self.table.mark_path_used(
                        &self.enclosing,
                        self.module,
                        &RefPath::single(&name),
                        false,
                    );
                }
                if let Some((_, subpat)) = &node.subpat {
                    self.bind_pat_inner(subpat);
                }
            }
            // A path in a pattern names a struct, a variant or a `const`; it
            // is a use, and only the leaves under it can bind.
            syn::Pat::TupleStruct(node) => {
                for attr in &node.attrs {
                    self.visit_attribute(attr);
                }
                if let Some(qself) = &node.qself {
                    self.visit_qself(qself);
                }
                self.visit_path(&node.path);
                for elem in &node.elems {
                    self.bind_pat_inner(elem);
                }
            }
            syn::Pat::Struct(node) => {
                for attr in &node.attrs {
                    self.visit_attribute(attr);
                }
                if let Some(qself) = &node.qself {
                    self.visit_qself(qself);
                }
                self.visit_path(&node.path);
                for field in &node.fields {
                    self.bind_pat_inner(&field.pat);
                }
            }
            syn::Pat::Type(node) => {
                for attr in &node.attrs {
                    self.visit_attribute(attr);
                }
                self.visit_type(&node.ty);
                self.bind_pat_inner(&node.pat);
            }
            syn::Pat::Tuple(node) => node.elems.iter().for_each(|e| self.bind_pat_inner(e)),
            syn::Pat::Slice(node) => node.elems.iter().for_each(|e| self.bind_pat_inner(e)),
            // Every alternative of an or-pattern binds the same names, so
            // walking them all is the same set with the uses in each kept.
            syn::Pat::Or(node) => node.cases.iter().for_each(|c| self.bind_pat_inner(c)),
            syn::Pat::Reference(node) => self.bind_pat_inner(&node.pat),
            syn::Pat::Paren(node) => self.bind_pat_inner(&node.pat),
            // `_`, `..`, literals, ranges, `const` blocks and macro patterns
            // bind nothing; the ordinary walk resolves what they name.
            other => syn::visit::visit_pat(self, other),
        }
    }
}

impl<'ast> Visit<'ast> for RefWalker<'_> {
    /// An item is a scope boundary in both directions: it opens one for its
    /// own generics and parameters, and it cannot see the bindings around it.
    /// `fn outer() { let helper = 5; fn inner() { helper() } }` calls the
    /// module's `helper`, so the enclosing scopes are set aside rather than
    /// merely shadowed.
    fn visit_item(&mut self, node: &'ast syn::Item) {
        let values = std::mem::take(&mut self.value_scopes);
        let types = std::mem::take(&mut self.type_scopes);
        let pos = std::mem::replace(&mut self.pos, PathPos::Other);
        // An item is also where a use stops being the enclosing item's and
        // starts being this one's: `pub fn orphan() { helper(); }` names
        // `helper` on `orphan`'s behalf and on nobody else's.
        let from = self.item_referrer(node);
        self.within(from, |walker| {
            walker.in_scope(|walker| syn::visit::visit_item(walker, node));
        });
        self.pos = pos;
        self.value_scopes = values;
        self.type_scopes = types;
    }

    /// A member of an `impl` or `trait` opens a scope for its own generics
    /// and parameters, on top of the block's — `impl<T> Foo<T>` is in scope
    /// throughout every method.
    fn visit_impl_item(&mut self, node: &'ast syn::ImplItem) {
        self.in_scope(|walker| syn::visit::visit_impl_item(walker, node));
    }

    fn visit_trait_item(&mut self, node: &'ast syn::TraitItem) {
        self.in_scope(|walker| syn::visit::visit_trait_item(walker, node));
    }

    fn visit_foreign_item(&mut self, node: &'ast syn::ForeignItem) {
        self.in_scope(|walker| syn::visit::visit_foreign_item(walker, node));
    }

    /// Generic parameters bind for the whole item they are declared on —
    /// signature, `where` clause and body alike — so they are recorded into
    /// the scope that item opened, before the bounds and defaults that can
    /// already mention them. Type parameters are types; const parameters are
    /// values (`[u8; N]`); lifetimes name no item and are ignored.
    fn visit_generics(&mut self, node: &'ast syn::Generics) {
        for param in &node.params {
            match param {
                syn::GenericParam::Type(param) => self.bind_type(param.ident.to_string()),
                syn::GenericParam::Const(param) => self.bind_value(param.ident.to_string()),
                syn::GenericParam::Lifetime(_) => {}
            }
        }
        syn::visit::visit_generics(self, node);
    }

    /// Parameters bind into the scope the item opened, so they cover the body
    /// as well as the rest of the signature.
    fn visit_signature(&mut self, node: &'ast syn::Signature) {
        self.visit_generics(&node.generics);
        for input in &node.inputs {
            match input {
                syn::FnArg::Receiver(receiver) => self.visit_receiver(receiver),
                syn::FnArg::Typed(arg) => {
                    for attr in &arg.attrs {
                        self.visit_attribute(attr);
                    }
                    self.visit_type(&arg.ty);
                    self.bind_pat(&arg.pat);
                }
            }
        }
        if let Some(variadic) = &node.variadic {
            self.visit_variadic(variadic);
        }
        self.visit_return_type(&node.output);
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        self.in_scope(|walker| syn::visit::visit_block(walker, node));
    }

    /// The initializer is resolved before the pattern binds, so `let x =
    /// x();` still names the item `x`; so is the `else` block, which runs
    /// where the binding does not exist.
    fn visit_local(&mut self, node: &'ast syn::Local) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        if let Some(init) = &node.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        self.bind_pat(&node.pat);
    }

    /// An arm's pattern binds in its guard and its body, and nowhere else.
    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.in_scope(|walker| {
            walker.bind_pat(&node.pat);
            if let Some((_, guard)) = &node.guard {
                walker.visit_expr(guard);
            }
            walker.visit_expr(&node.body);
        });
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        // A closure sees the bindings around it, so this scope is stacked on
        // them rather than replacing them.
        self.in_scope(|walker| {
            for input in &node.inputs {
                walker.bind_pat(input);
            }
            walker.visit_return_type(&node.output);
            walker.visit_expr(&node.body);
        });
    }

    /// The iterator expression is outside the binding: `for x in x()` calls
    /// the item.
    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.visit_expr(&node.expr);
        self.in_scope(|walker| {
            walker.bind_pat(&node.pat);
            walker.visit_block(&node.body);
        });
    }

    /// `if let` binds in the `then` branch only — never in the `else`, and
    /// never after the `if` — so the scope wraps exactly those two.
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.in_scope(|walker| {
            walker.visit_expr(&node.cond);
            walker.visit_block(&node.then_branch);
        });
        if let Some((_, alternative)) = &node.else_branch {
            self.visit_expr(alternative);
        }
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.in_scope(|walker| {
            walker.visit_expr(&node.cond);
            walker.visit_block(&node.body);
        });
    }

    /// The scrutinee of `if let`/`while let` is resolved before the pattern
    /// binds, exactly as a `let` statement's initializer is. The binding lands
    /// in the scope the enclosing `if` or `while` opened; a `let` chain's
    /// later links see the earlier ones, which is what Rust does.
    fn visit_expr_let(&mut self, node: &'ast syn::ExprLet) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        self.visit_expr(&node.expr);
        self.bind_pat(&node.pat);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.with_pos(PathPos::Expr, |walker| {
            syn::visit::visit_expr_path(walker, node);
        });
    }

    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        self.with_pos(PathPos::Type, |walker| {
            syn::visit::visit_type_path(walker, node);
        });
    }

    /// `T: Bound` holds a bare `syn::Path` rather than a `TypePath`, so the
    /// namespace has to be set here too.
    fn visit_trait_bound(&mut self, node: &'ast syn::TraitBound) {
        self.with_pos(PathPos::Type, |walker| {
            syn::visit::visit_trait_bound(walker, node);
        });
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        let Some((_, items)) = &node.content else {
            return;
        };
        let outer_module = self.module;
        self.module_path.push(node.ident.to_string());
        if let Some(&inner) = self
            .table
            .by_path
            .get(&(self.krate, self.module_path.clone()))
        {
            self.module = inner;
        }
        for item in items {
            self.visit_item(item);
        }
        self.module_path.pop();
        self.module = outer_module;
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        // A `use` is a reference to whatever it imports, made on behalf of the
        // name it binds: the import is alive only where something goes through
        // it, and only then is what it names. Attributing the reference to the
        // file instead would make every `use` in the workspace a root, and
        // nothing imported could ever be found dead.
        //
        // A glob and a `use x as _;` bind no name to hang it on — the second
        // imports a trait's methods for a dispatch we cannot see at all — so
        // both stay unconditional.
        let absolute = node.leading_colon.is_some();
        let mut leaves = Vec::new();
        flatten_use(&node.tree, &mut Vec::new(), &mut leaves);
        for leaf in leaves {
            let from = match &leaf.alias {
                Some(alias) => self.defined_at(leaf.line, alias),
                None => Referrer::Root,
            };
            let path = RefPath {
                absolute,
                segments: leaf.segments,
            };
            self.table.mark_path_used(&from, self.module, &path, true);
        }
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let path = RefPath::from_syn(node);
        // `Foo::new()` or `Foo { .. }` written inside `impl Foo` says nothing
        // about whether anyone uses `Foo`; it is the same self-reference as
        // the `impl` header itself.
        let names_self = node.leading_colon.is_none()
            && self.impl_self.as_deref() == path.segments.first().map(String::as_str);
        if !names_self && !self.shadowed(&path) {
            self.table
                .mark_path_used(&self.enclosing, self.module, &path, false);
        }
        // Generic arguments inside the segments are paths of their own.
        syn::visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // A macro name is in the macro namespace, which no binding tracked
        // here reaches. It matters because a macro can sit inside a path's
        // generic arguments (`Vec<thing!()>`, `take::<{ thing!() }>()`) and
        // would otherwise inherit that path's namespace, letting a generic
        // parameter or a local of the same name suppress it.
        self.with_pos(PathPos::Other, |walker| {
            syn::visit::visit_macro(walker, node);
        });
        self.table.mark_tokens_used(&node.tokens);
    }

    fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
        // Attribute arguments are resolved by macro expansion, which we do
        // not perform, so every identifier counts as a use.
        match &node.meta {
            syn::Meta::Path(path) => self.table.mark_path_names_used(path),
            syn::Meta::List(list) => {
                self.table.mark_path_names_used(&list.path);
                self.table.mark_attr_tokens_used(&list.tokens);
            }
            syn::Meta::NameValue(nv) => {
                self.table.mark_path_names_used(&nv.path);
                self.visit_expr(&nv.value);
            }
        }
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        for attr in &node.attrs {
            self.visit_attribute(attr);
        }
        let owner = self.impl_owner(node);
        self.within(owner, |walker| walker.visit_impl_body(node));
    }
}

impl RefWalker<'_> {
    /// What an `impl` block hangs off, for attributing the uses written
    /// inside it.
    ///
    /// A block has no definition of its own, so it is alive exactly when its
    /// self type is — nothing can call a method on a type nothing can name —
    /// or, for a trait impl we can resolve, when the trait is: a trait's
    /// methods are reached through a `dyn` or a bound with the implementing
    /// type never spelled, and that dispatch is invisible to a syntactic tool.
    ///
    /// A self type we cannot pin to a definition here is
    /// [`Referrer::Root`], and that covers most impls in most codebases:
    /// a foreign type (`impl Trait for Vec<T>`), a generic parameter (a
    /// blanket `impl<T> Trait for T`), a tuple, a reference, an array. Each is
    /// a case where claiming the block is dead would be claiming something we
    /// have no evidence for.
    fn impl_owner(&self, node: &syn::ItemImpl) -> Referrer {
        let syn::Type::Path(ty) = &*node.self_ty else {
            return Referrer::Root;
        };
        if ty.qself.is_some() {
            return Referrer::Root;
        }
        // `impl<T> Trait for T` names the block's own parameter, never a
        // module item that happens to share its name. The parameters are read
        // straight off the block because the scope frame that would hold them
        // is opened by `visit_generics`, which has to run *after* this — the
        // bounds it walks are uses attributed to the answer computed here.
        let head = ty.path.segments.first().map(|segment| &segment.ident);
        let names_own_param = ty.path.leading_colon.is_none()
            && ty.path.segments.len() == 1
            && node.generics.params.iter().any(|param| match param {
                syn::GenericParam::Type(param) => Some(&param.ident) == head,
                _ => false,
            });
        if names_own_param {
            return Referrer::Root;
        }
        let Some(mut owners) = self
            .table
            .owner_defs(self.module, &RefPath::from_syn(&ty.path))
        else {
            return Referrer::Root;
        };
        if let Some((_, path, _)) = &node.trait_
            && let Some(from_trait) = self.table.owner_defs(self.module, &RefPath::from_syn(path))
        {
            owners.extend(from_trait);
        }
        if owners.is_empty() {
            Referrer::Root
        } else {
            Referrer::Defs(owners)
        }
    }

    /// The rest of an `impl` block, once [`RefWalker::impl_owner`] has decided
    /// who the paths in it belong to.
    fn visit_impl_body(&mut self, node: &syn::ItemImpl) {
        self.visit_generics(&node.generics);
        // Implementing a trait is a use of the trait.
        if let Some((_, path, _)) = &node.trait_ {
            self.visit_path(path);
        }
        // But an `impl` block is not a use of the type it belongs to: a type
        // whose only mention is its own impl is still unreferenced. Generic
        // arguments in the self type are real uses (`impl Foo<Bar>`).
        let mut self_name = None;
        match &*node.self_ty {
            syn::Type::Path(ty) if ty.qself.is_none() => {
                // A bare `impl Foo` names the type in its head segment, so a
                // body path starting with `Foo` is the self-reference.
                //
                // For a qualified `impl crate::foo::Bar` the head segment is
                // only a qualifier — suppressing every `crate::` path in the
                // body would hide real uses — but a body path can still spell
                // the type by its bare last segment. That is the same
                // self-reference, and only when the bare name provably means
                // this very item: it may instead be another `Bar` that a
                // `use` brought into scope, and suppressing that would report
                // a live item as dead.
                if ty.path.leading_colon.is_none() && ty.path.segments.len() == 1 {
                    self_name = Some(ty.path.segments[0].ident.to_string());
                } else if let Some(last) = ty.path.segments.last() {
                    let name = last.ident.to_string();
                    let path = RefPath::from_syn(&ty.path);
                    if self.table.bare_name_means(self.module, &path, &name) {
                        self_name = Some(name);
                    }
                }
                for segment in &ty.path.segments {
                    self.visit_path_arguments(&segment.arguments);
                }
            }
            other => self.visit_type(other),
        }
        let outer_self = std::mem::replace(&mut self.impl_self, self_name);
        for item in &node.items {
            self.visit_impl_item(item);
        }
        self.impl_self = outer_self;
    }
}

/// Collects `use` declarations nested inside item bodies.
struct NestedUses<'a> {
    table: &'a mut SymbolTable,
    module: usize,
    file: &'a ParsedFile,
    test_context: bool,
}

impl<'ast> Visit<'ast> for NestedUses<'_> {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.table
            .add_use(node, self.module, self.file, false, self.test_context);
    }
}

/// One name bound (or glob imported) by a `use` declaration.
struct UseLeaf {
    /// `None` for a glob or for `use x as _;`, which bind no name.
    alias: Option<String>,
    segments: Vec<String>,
    line: usize,
    glob: bool,
}

fn flatten_use(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut Vec<UseLeaf>) {
    match tree {
        syn::UseTree::Path(node) => {
            prefix.push(node.ident.to_string());
            flatten_use(&node.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(node) => {
            let name = node.ident.to_string();
            let line = node.ident.span().start().line;
            // `use a::b::{self}` imports `b` itself, not a name `self`.
            if name == "self" {
                out.push(UseLeaf {
                    alias: prefix.last().cloned(),
                    segments: prefix.clone(),
                    line,
                    glob: false,
                });
            } else {
                let mut segments = prefix.clone();
                segments.push(name.clone());
                out.push(UseLeaf {
                    alias: Some(name),
                    segments,
                    line,
                    glob: false,
                });
            }
        }
        syn::UseTree::Rename(node) => {
            let name = node.ident.to_string();
            let alias = node.rename.to_string();
            let mut segments = prefix.clone();
            if name != "self" {
                segments.push(name);
            }
            out.push(UseLeaf {
                // `use x as _;` binds nothing that can be referenced.
                alias: (alias != "_").then_some(alias),
                segments,
                line: node.ident.span().start().line,
                glob: false,
            });
        }
        // A glob binds no name of its own, so it is never reported and needs
        // no location.
        syn::UseTree::Glob(_) => out.push(UseLeaf {
            alias: None,
            segments: prefix.clone(),
            line: 0,
            glob: true,
        }),
        syn::UseTree::Group(group) => {
            for item in &group.items {
                flatten_use(item, prefix, out);
            }
        }
    }
}

/// The name, kind, attributes, and visibility of an item that can be defined
/// at module level and reported. Items with no name of their own (`impl`,
/// `use`, `extern crate`, macro definitions) return `None`.
fn describe(
    item: &syn::Item,
) -> Option<(&syn::Ident, DefKind, &[syn::Attribute], &syn::Visibility)> {
    match item {
        syn::Item::Fn(i) => Some((&i.sig.ident, DefKind::Fn, &i.attrs, &i.vis)),
        syn::Item::Struct(i) => Some((&i.ident, DefKind::Struct, &i.attrs, &i.vis)),
        syn::Item::Enum(i) => Some((&i.ident, DefKind::Enum, &i.attrs, &i.vis)),
        syn::Item::Trait(i) => Some((&i.ident, DefKind::Trait, &i.attrs, &i.vis)),
        syn::Item::Type(i) => Some((&i.ident, DefKind::TypeAlias, &i.attrs, &i.vis)),
        syn::Item::Const(i) => Some((&i.ident, DefKind::Const, &i.attrs, &i.vis)),
        syn::Item::Static(i) => Some((&i.ident, DefKind::Static, &i.attrs, &i.vis)),
        syn::Item::Union(i) => Some((&i.ident, DefKind::Union, &i.attrs, &i.vis)),
        _ => None,
    }
}

/// Whether `attr` is one of `names`.
///
/// Two forgivenesses, both of which can only keep an item alive. The *last*
/// path segment decides, so `#[tokio::test]` and `#[async_std::test]` are the
/// test entry points they are. And `unsafe(...)` is unwrapped: edition 2024
/// spells the linker exports `#[unsafe(no_mangle)]`, which parses as an
/// attribute named `unsafe` whose tokens hold the real one, so reading the
/// outer path alone would miss every export written the way current Rust
/// requires.
fn attr_is(attr: &syn::Attribute, names: &[&str]) -> bool {
    if attr.path().is_ident("unsafe")
        && let syn::Meta::List(list) = &attr.meta
        && let Some(TokenTree::Ident(inner)) = list.tokens.clone().into_iter().next()
    {
        return names.contains(&inner.to_string().as_str());
    }
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| names.contains(&segment.ident.to_string().as_str()))
}

/// Attributes that mark an item as used externally or deliberately kept.
fn has_skip_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr_is(attr, &["no_mangle", "used", "export_name"]) {
            return true;
        }
        if attr_is(attr, &["allow", "expect"])
            && let syn::Meta::List(list) = &attr.meta
        {
            let lints = list.tokens.to_string();
            return lints.contains("dead_code") || lints.contains("unused");
        }
        false
    })
}

/// Attributes that say something outside the source reaches an item, so
/// nothing inside the workspace has to.
///
/// These are the entry points of [`SymbolTable::reachable`]'s root set. Each
/// is a name a linker, the compiler, or a test harness knows and no path in
/// the workspace ever spells; missing one would report a live item, and
/// everything it reaches, as dead. The list is deliberately generous —
/// including one an unresolved attribute macro merely *might* honour costs a
/// finding, and leaving one out costs precision.
/// The entry points a test harness calls, and nothing else does.
///
/// These are the whole of the difference between the two root sets: a build
/// with `[cfg] test = false` compiles neither, so anything only these reach is
/// reached only by test code. `#[bench]` is here with `#[test]` because
/// `cargo bench` is no more a consumer of the crate than `cargo test` is.
const TEST_ENTRY_POINT_ATTRS: &[&str] = &["test", "bench"];

/// Entry points every build has, whatever it does with the tests.
const ENTRY_POINT_ATTRS: &[&str] = &[
    // Named by the linker rather than by any path.
    "no_mangle",
    "export_name",
    "used",
    "link_section",
    // Named by the compiler.
    "main",
    "start",
    "panic_handler",
    "global_allocator",
    "alloc_error_handler",
    "lang",
    "proc_macro",
    "proc_macro_derive",
    "proc_macro_attribute",
    // Registered before `main` by generated code we never see.
    "ctor",
    "dtor",
];

/// Whether an item's attributes make it a root, and whether a build without
/// tests is one of the things that reaches it.
///
/// The `dead_code` opt-outs count, because `#[allow(dead_code)]` is the author
/// saying the item is kept on purpose and an item kept on purpose keeps what
/// it names. Only those, though: [`has_skip_attr`] answers yes to any
/// `allow`/`expect` whose tokens merely *contain* `unused`, which is right for
/// suppressing a report — the author has said they do not want to hear about
/// this item — and much too broad for rooting. `#[allow(unused_variables)]`
/// says nothing about whether the function is reached, and rooting on it would
/// silence a whole cascade under one of the most common attributes in Rust.
///
/// `test_context` is what the *target* and the file say, and it only ever
/// moves an entry point from [`EntryPoint::NonTest`] to [`EntryPoint::Test`]:
/// a `#[no_mangle]` in an example is exported into a binary `cargo test`
/// builds and nothing else runs. A `#[test]` is a test entry point wherever it
/// is written, which is the other half of that agreement.
fn entry_point_attr(attrs: &[syn::Attribute], test_context: bool) -> EntryPoint {
    if attrs
        .iter()
        .any(|attr| attr_is(attr, TEST_ENTRY_POINT_ATTRS))
    {
        return EntryPoint::Test;
    }
    if attrs
        .iter()
        .any(|attr| attr_is(attr, ENTRY_POINT_ATTRS) || allows_lint(attr, &["dead_code", "unused"]))
    {
        return EntryPoint::of_context(test_context);
    }
    EntryPoint::None
}

/// Whether `attr` is an `allow` or `expect` listing one of `lints`.
///
/// Matched on the whole lint name rather than on a substring, which is the
/// difference between `unused` — the group that contains `dead_code` — and
/// `unused_variables`, which is about a parameter and not about the item.
fn allows_lint(attr: &syn::Attribute, lints: &[&str]) -> bool {
    if !attr_is(attr, &["allow", "expect"]) {
        return false;
    }
    let syn::Meta::List(list) = &attr.meta else {
        return false;
    };
    list.tokens
        .to_string()
        .split(',')
        .any(|lint| lints.contains(&lint.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crate whose files are `(module path, source)`; an empty module path
    /// is the crate root.
    fn unit(sources: &[(&str, &str)]) -> CrateUnit {
        CrateUnit {
            names: vec!["fixture".to_string()],
            test_code: false,
            files: sources
                .iter()
                .map(|(module, source)| ParsedFile {
                    path: PathBuf::from(format!("/ws/src/{module}.rs")),
                    ast: Some(syn::parse_file(source).expect("fixture source must parse")),
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

    /// The same crate under a name other crates can write, with its files in a
    /// directory of their own so two crates in one table cannot collide on the
    /// `(file, line, name)` site the report deduplicates by.
    fn named_unit(name: &str, sources: &[(&str, &str)]) -> CrateUnit {
        let mut unit = unit(sources);
        unit.names = vec![name.to_string()];
        for file in &mut unit.files {
            let module = file.module.join("/");
            file.path = PathBuf::from(format!("/ws/{name}/src/{module}.rs"));
        }
        unit
    }

    /// The reportable definitions in `sources` that no resolved path reaches,
    /// by name, in report order.
    fn unused_in(sources: &[(&str, &str)]) -> Vec<String> {
        let crates = [unit(sources)];
        let mut table = SymbolTable::build(&crates);
        table.record_references(&crates);
        table
            .unused_definitions(&PublicApi::default())
            .into_iter()
            .map(|def| def.name)
            .collect()
    }

    /// The same for a single crate-root file, which most scope cases are.
    fn unused_in_root(source: &str) -> Vec<String> {
        unused_in(&[("", source)])
    }

    /// The same for a crate nothing outside the workspace can name — a bin, a
    /// test, a bench. No item in one is a root by being `pub`, which is what
    /// makes a cascade visible at all: in a library the surface is rooted and
    /// the walk stops at the first `pub fn` under `pub` modules.
    fn unused_in_binary(sources: &[(&str, &str)]) -> Vec<String> {
        let crates = [CrateUnit {
            names: Vec::new(),
            test_code: false,
            files: unit(sources).files,
        }];
        let mut table = SymbolTable::build(&crates);
        table.record_references(&crates);
        table
            .unused_definitions(&PublicApi::default())
            .into_iter()
            .map(|def| def.name)
            .collect()
    }

    /// The single-file case, which most reachability cases are.
    fn unused_in_binary_root(source: &str) -> Vec<String> {
        unused_in_binary(&[("", source)])
    }

    /// The definitions in `crates` only test code reaches, by name.
    fn test_only_in(crates: &[CrateUnit]) -> Vec<String> {
        let mut table = SymbolTable::build(crates);
        table.record_references(crates);
        table
            .test_only_definitions(&PublicApi::default())
            .into_iter()
            .map(|def| def.name)
            .collect()
    }

    /// The same for one library crate, where the public surface is a root —
    /// which is why every case below puts the item in a *private* module.
    fn test_only_in_library(sources: &[(&str, &str)]) -> Vec<String> {
        test_only_in(&[unit(sources)])
    }

    fn module_at(table: &SymbolTable, path: &[&str]) -> usize {
        let path: Vec<String> = path.iter().map(|s| (*s).to_string()).collect();
        *table
            .by_path
            .get(&(0, path.clone()))
            .unwrap_or_else(|| panic!("no module at {path:?}"))
    }

    /// Two crates answering to the same name (a dependency rename colliding
    /// with another crate's name) must not be guessed between: resolving
    /// through the wrong one would report the right one's items as dead.
    #[test]
    fn a_crate_name_claimed_twice_falls_back_to_the_conservative_rule() {
        let mut left = unit(&[("", "pub fn contested() {}\n")]);
        left.names = vec!["left".to_string(), "shared".to_string()];
        let mut right = unit(&[("", "pub fn contested() {}\n")]);
        right.names = vec!["right".to_string(), "shared".to_string()];
        let caller = CrateUnit {
            names: Vec::new(),
            test_code: false,
            files: unit(&[("", "fn go() { shared::contested(); }\n")]).files,
        };

        let crates = [left, right, caller];
        let mut table = SymbolTable::build(&crates);
        assert!(
            table.ambiguous_externs.contains("shared"),
            "`shared` names two crates and must be marked ambiguous"
        );
        table.record_references(&crates);

        let unused = table.unused_definitions(&PublicApi::default());
        assert!(
            unused.is_empty(),
            "an ambiguous head segment keeps every candidate alive: {:?}",
            unused
                .iter()
                .map(|def| def.name.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_glob_into_a_workspace_module_is_expanded() {
        let table = SymbolTable::build(&[unit(&[
            ("", "mod source;\nuse source::*;\n"),
            ("source", "pub fn item() {}\n"),
        ])]);

        let root = module_at(&table, &[]);
        assert!(
            !table.modules[root].opaque,
            "a glob we can follow leaves no hole in the scope"
        );
        assert_eq!(
            table.modules[root].glob_sources,
            vec![module_at(&table, &["source"])]
        );
        assert!(
            matches!(table.lookup(root, "item"), Step::Defs(_)),
            "the glob brings `item` into the root's scope"
        );
    }

    /// `mod tests { use super::*; }` is the most common glob in Rust code:
    /// it has to resolve, or every test module in the workspace goes opaque
    /// and hides findings.
    #[test]
    fn a_super_glob_resolves_to_the_parent_module() {
        let table = SymbolTable::build(&[unit(&[(
            "",
            "pub fn item() {}\nmod tests {\n    use super::*;\n}\n",
        )])]);

        let tests = module_at(&table, &["tests"]);
        assert!(!table.modules[tests].opaque);
        assert_eq!(
            table.modules[tests].glob_sources,
            vec![module_at(&table, &[])]
        );
    }

    #[test]
    fn a_glob_leading_outside_the_workspace_makes_the_module_opaque() {
        let table = SymbolTable::build(&[unit(&[("", "use other_crate::prelude::*;\n")])]);

        let root = module_at(&table, &[]);
        assert!(table.modules[root].opaque);
        assert!(
            matches!(table.lookup(root, "anything"), Step::Unknown),
            "an unknown name in an opaque scope must not read as absent"
        );
    }

    #[test]
    fn opacity_propagates_through_a_resolved_glob() {
        // The root's glob is followed, but it leads into a module that is
        // itself opaque, so the root cannot claim to know its own scope.
        let table = SymbolTable::build(&[unit(&[
            ("", "mod source;\nuse source::*;\n"),
            ("source", "use other_crate::*;\n"),
        ])]);

        let root = module_at(&table, &[]);
        assert!(matches!(table.lookup(root, "anything"), Step::Unknown));
    }

    // -- lexical scopes ----------------------------------------------------

    #[test]
    fn a_local_binding_shadows_a_module_item_of_the_same_name() {
        assert_eq!(
            unused_in_root(
                "pub fn helper() -> u32 { 1 }\npub fn entry() -> u32 { let helper = 5; helper }\n"
            ),
            vec!["helper", "entry"]
        );
    }

    #[test]
    fn a_function_parameter_shadows_a_module_item_in_the_body() {
        assert_eq!(
            unused_in_root(
                "pub fn width() -> u32 { 1 }\npub fn entry(width: u32) -> u32 { width }\n"
            ),
            vec!["width", "entry"]
        );
    }

    /// `let x = x();` is the ordering case: the binding starts after the
    /// initializer, so the call still names the item.
    #[test]
    fn a_binding_does_not_shadow_the_initializer_that_creates_it() {
        assert_eq!(
            unused_in_root(
                "pub fn seeded() -> u32 { 1 }\npub fn entry() -> u32 { let seeded = seeded(); seeded }\n"
            ),
            vec!["entry"]
        );
    }

    /// Rust resolves values and types apart, so a `let` binding must not
    /// silence a type of the same name: doing so would report a live item as
    /// dead, which is the failure this whole phase is shaped around.
    #[test]
    fn a_value_binding_does_not_shadow_a_type_of_the_same_name() {
        assert_eq!(
            unused_in_root(
                "pub struct Cfg { pub n: u32 }\npub fn entry() -> u32 { let mut Cfg = 1; Cfg += 1; let _t: Cfg = Default::default(); Cfg }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn a_generic_type_parameter_shadows_a_type_of_the_same_name() {
        assert_eq!(
            unused_in_root("pub struct Marker;\npub fn wrap<Marker>(m: Marker) -> Marker { m }\n"),
            vec!["Marker", "wrap"]
        );
    }

    /// The other half of the split: a type parameter lives in the type
    /// namespace only, so an *expression* of that name still names the item.
    #[test]
    fn a_generic_type_parameter_does_not_shadow_an_expression_of_the_same_name() {
        assert_eq!(
            unused_in_root("pub const N: usize = 4;\npub fn take<N>(_v: N) -> usize { N }\n"),
            vec!["take"]
        );
    }

    #[test]
    fn a_generic_parameter_binds_only_for_the_item_declaring_it() {
        assert_eq!(
            unused_in_root(
                "pub struct Marker;\npub fn wrap<Marker>(m: Marker) -> Marker { m }\npub fn plain(m: Marker) -> Marker { m }\n"
            ),
            vec!["wrap", "plain"]
        );
    }

    #[test]
    fn a_tuple_struct_pattern_names_the_struct_and_binds_only_its_leaves() {
        assert_eq!(
            unused_in_root(
                "pub struct Pair(pub u32);\npub fn value() -> u32 { 1 }\npub fn entry() -> u32 { let Pair(value) = Default::default(); value }\n"
            ),
            vec!["value", "entry"]
        );
    }

    #[test]
    fn a_struct_pattern_names_the_struct_and_binds_only_its_fields() {
        assert_eq!(
            unused_in_root(
                "pub struct Wrap { pub inner: u32 }\npub fn inner() -> u32 { 1 }\npub fn entry() -> u32 { let Wrap { inner } = Default::default(); inner }\n"
            ),
            vec!["inner", "entry"]
        );
    }

    /// A bare name in pattern position is a unit-struct pattern rather than a
    /// binding — Rust rejects `let Unit = ..;` beside `struct Unit;` outright
    /// (E0530) — so it has to be resolved as a use.
    #[test]
    fn a_bare_pattern_naming_a_unit_struct_is_a_use_rather_than_a_binding() {
        assert_eq!(
            unused_in_root(
                "pub struct Unit;\npub fn entry(v: u32) -> u32 { match v { Unit => 1, _ => 0 } }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn a_bare_pattern_naming_a_const_is_a_use_rather_than_a_binding() {
        assert_eq!(
            unused_in_root(
                "pub const LIMIT: u32 = 4;\npub fn entry(v: u32) -> u32 { match v { LIMIT => 0, other => other } }\n"
            ),
            vec!["entry"]
        );
    }

    /// The same question asked where the scope has holes: an unfollowable
    /// glob could be bringing a `const` of that name in, so the pattern is
    /// resolved as a use and reaches every item with the name.
    #[test]
    fn a_bare_pattern_in_an_opaque_scope_is_a_use_rather_than_a_binding() {
        assert_eq!(
            unused_in(&[
                ("", "mod holder;\nmod user;\n"),
                ("holder", "pub const LIMIT: u32 = 1;\n"),
                (
                    "user",
                    "use other_crate::*;\npub fn entry(v: u32) -> u32 { match v { LIMIT => 0, other => other } }\n"
                ),
            ]),
            vec!["entry"]
        );
    }

    #[test]
    fn a_binding_does_not_outlive_its_block() {
        assert_eq!(
            unused_in_root(
                "pub fn scoped() -> u32 { 1 }\npub fn entry() -> u32 { { let scoped = 2; let _ = scoped; } scoped() }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn a_match_arms_binding_does_not_reach_the_other_arms() {
        assert_eq!(
            unused_in_root(
                "pub fn armed() -> u32 { 1 }\npub fn entry(v: Option<u32>) -> u32 { match v { Some(armed) => armed, None => armed() } }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn a_let_else_block_runs_before_the_binding_exists() {
        assert_eq!(
            unused_in_root(
                "pub fn fallback() -> u32 { 1 }\npub fn entry(v: Option<u32>) -> u32 { let Some(fallback) = v else { return fallback() }; fallback }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn an_if_let_binding_does_not_reach_the_else_branch() {
        assert_eq!(
            unused_in_root(
                "pub fn other() -> u32 { 1 }\npub fn entry(v: Option<u32>) -> u32 { if let Some(other) = v { other } else { other() } }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn a_closure_parameter_binds_only_inside_the_closure() {
        assert_eq!(
            unused_in_root(
                "pub fn cb() -> u32 { 1 }\npub fn entry() -> u32 { let f = |cb: u32| cb; f(1) + cb() }\n"
            ),
            vec!["entry"]
        );
    }

    #[test]
    fn a_for_loop_pattern_does_not_shadow_the_iterator_expression() {
        assert_eq!(
            unused_in_root(
                "pub fn source() -> Vec<u32> { Vec::new() }\npub fn entry() -> u32 { let mut n = 0; for source in source() { n += source; } n }\n"
            ),
            vec!["entry"]
        );
    }

    /// A local shadows a name, never a path through it.
    #[test]
    fn a_qualified_path_is_never_shadowed_by_a_local() {
        assert_eq!(
            unused_in_root(
                "pub mod deep { pub fn thing() -> u32 { 1 } }\npub fn entry() -> u32 { let deep = 2; deep + deep::thing() }\n"
            ),
            vec!["entry"]
        );
    }

    /// Rust rejects reaching an enclosing function's local from an item
    /// nested in it (E0434), so no compiling program depends on the answer.
    /// Starting each item from an empty scope is the direction that keeps the
    /// module's item alive rather than reporting it dead.
    #[test]
    fn an_item_nested_in_a_function_does_not_see_that_functions_locals() {
        assert_eq!(
            unused_in_root(
                "pub fn shared() -> u32 { 1 }\npub fn entry() -> u32 { let shared = 2; fn inner() -> u32 { shared() } shared + inner() }\n"
            ),
            vec!["entry"]
        );
    }

    /// The same boundary in the type namespace: an outer generic parameter is
    /// not in scope inside a nested item (E0401), so a type of that name
    /// there is the module's.
    #[test]
    fn an_item_nested_in_a_function_does_not_see_that_functions_generics() {
        assert_eq!(
            unused_in_root(
                "pub struct Held;\npub fn entry<Held>(v: Held) -> Held { struct Inner(Held); v }\n"
            ),
            vec!["entry"]
        );
    }

    // -- reachability ------------------------------------------------------

    /// The cascade from #21: nothing names `orphan`, so nothing names
    /// `helper` either however plainly `orphan` spells it.
    #[test]
    fn an_item_reached_only_from_an_unreached_item_is_reported() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn helper() -> u32 { 1 }\npub fn orphan() -> u32 { helper() }\n"
            ),
            vec!["helper", "orphan"]
        );
    }

    /// The case a reference count can never express: both are referenced,
    /// both are dead, and rerunning finds neither.
    #[test]
    fn a_mutually_recursive_pair_nothing_reaches_is_reported_in_full() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn ping(n: u32) -> u32 { pong(n) }\npub fn pong(n: u32) -> u32 { ping(n) }\n"
            ),
            vec!["ping", "pong"]
        );
    }

    /// The finding this must not invent: an entry point carries all the way
    /// down its chain.
    #[test]
    fn a_chain_from_an_entry_point_stays_quiet() {
        assert!(
            unused_in_binary_root(
                "pub fn leaf() -> u32 { 1 }\npub fn middle() -> u32 { leaf() }\nfn main() { let _ = middle(); }\n"
            )
            .is_empty()
        );
    }

    /// An opaque mention is a root, not an edge: it is a use of every item of
    /// that name precisely because we cannot see which one it meant, so we
    /// certainly cannot say whose behalf it was made on.
    #[test]
    fn an_opaque_mention_is_a_root_rather_than_an_edge() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn mentioned() -> u32 { 1 }\npub fn dead_caller() { println!(\"{}\", mentioned as usize); }\n"
            ),
            vec!["dead_caller"],
            "the caller is dead; the macro input that names `mentioned` is not evidence we can read"
        );
    }

    /// Same rule for an attribute's arguments, which a macro we do not expand
    /// is what resolves.
    #[test]
    fn an_attribute_argument_is_a_root_rather_than_an_edge() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn target() -> u32 { 1 }\n#[some_attr(target)]\npub fn dead_holder() {}\n"
            ),
            vec!["dead_holder"]
        );
    }

    /// A `use` names its target on the bound name's behalf, so an import
    /// nothing goes through stops keeping what it imports alive. Attributing
    /// it to the file instead would make every `use` in the workspace a root.
    #[test]
    fn an_import_nothing_goes_through_does_not_keep_its_target_alive() {
        assert_eq!(
            unused_in_binary(&[
                (
                    "",
                    "mod inner;\nuse inner::thing;\npub fn dead_caller() -> u32 { thing() }\n"
                ),
                ("inner", "pub fn thing() -> u32 { 1 }\n"),
            ]),
            vec!["dead_caller", "thing"]
        );
    }

    #[test]
    fn an_import_something_reached_goes_through_keeps_its_target_alive() {
        assert!(
            unused_in(&[
                (
                    "",
                    "mod inner;\nuse inner::thing;\n#[test]\nfn t() { thing(); }\n"
                ),
                ("inner", "pub fn thing() -> u32 { 1 }\n"),
            ])
            .is_empty()
        );
    }

    /// An `impl` block has no definition of its own, so its body belongs to
    /// the type it is written for: nothing can call a method on a type nothing
    /// can name.
    #[test]
    fn an_impl_blocks_body_belongs_to_its_self_type() {
        assert_eq!(
            unused_in_binary_root(
                "pub struct Owner;\npub fn helper() -> u32 { 1 }\nimpl Owner { fn go() -> u32 { helper() } }\n"
            ),
            vec!["Owner", "helper"]
        );
        assert!(
            unused_in_binary_root(
                "pub struct Owner;\npub fn helper() -> u32 { 1 }\nimpl Owner { fn go() -> u32 { helper() } }\nfn main() { let _ = Owner; }\n"
            )
            .is_empty(),
            "reaching the type reaches its impls"
        );
    }

    /// ...and to the trait as well, when we can resolve it. A trait's methods
    /// are called through a `dyn` or a bound with the implementing type never
    /// spelled, and that dispatch is invisible to a syntactic tool.
    #[test]
    fn an_impl_of_a_reached_trait_keeps_its_body_alive() {
        assert_eq!(
            unused_in_binary_root(
                "pub trait Marker { fn m() -> u32; }\npub struct Owner;\npub fn helper() -> u32 { 1 }\n\
                 impl Marker for Owner { fn m() -> u32 { helper() } }\nfn take<T: Marker>() {}\nfn main() { take::<u32>(); }\n"
            ),
            vec!["Owner"],
            "the trait is reached, so the impl is, so its body is"
        );
    }

    /// A self type outside the workspace leaves nothing to hang the block off,
    /// so its body is unconditional. This covers most impls in most codebases
    /// — foreign types, tuples, references, arrays.
    #[test]
    fn an_impl_for_a_type_outside_the_workspace_is_a_root() {
        assert!(
            unused_in_binary_root(
                "pub fn helper() -> u32 { 1 }\nimpl Outside { fn go() -> u32 { helper() } }\n"
            )
            .is_empty()
        );
    }

    /// `impl<T> Marker for T` names the block's own parameter, not a module
    /// item that happens to share its name. Reading it as the item would hand
    /// the whole blanket impl to a type nobody uses.
    #[test]
    fn a_blanket_impl_over_its_own_parameter_is_not_owned_by_a_namesake_item() {
        assert_eq!(
            unused_in_binary_root(
                "pub struct T;\npub trait Marker { fn m() -> u32; }\npub fn helper() -> u32 { 1 }\n\
                 impl<T> Marker for T { fn m() -> u32 { helper() } }\n"
            ),
            vec!["T"],
            "only the struct nothing names is dead"
        );
    }

    /// A library's public surface is a root, because consumers we cannot see
    /// call it — but a root is not exempt from being reported. Both halves
    /// matter: the first is what keeps a library's API from cascading into a
    /// page of noise, the second is what keeps every finding Deadwood made
    /// before reachability existed.
    #[test]
    fn a_librarys_public_surface_is_a_root_and_is_still_reported_when_nothing_names_it() {
        assert_eq!(
            unused_in_root("pub fn helper() -> u32 { 1 }\npub fn surface() -> u32 { helper() }\n"),
            vec!["surface"],
            "`helper` is reached through the surface; `surface` itself has no caller"
        );
    }

    /// The line the surface rule draws is the one `is_externally_reachable`
    /// already drew for re-exports: a `pub` item behind a private module is
    /// not something outside code can name, so it is an ordinary node and the
    /// cascade runs through it. Open the module and both are roots.
    #[test]
    fn a_pub_item_behind_a_private_module_is_not_a_surface_root() {
        let inner = "pub fn helper() -> u32 { deeper() }\npub fn deeper() -> u32 { 1 }\n";
        assert_eq!(
            unused_in(&[("", "mod inner;\n"), ("inner", inner)]),
            vec!["helper", "deeper"],
            "nothing outside can name either, so `deeper` falls with `helper`"
        );
        assert_eq!(
            unused_in(&[("", "pub mod inner;\n"), ("inner", inner)]),
            vec!["helper"],
            "both are surface now, so `deeper` is reached however dead `helper` looks"
        );
    }

    // -- the public surface, through a `pub use` glob -----------------------
    //
    // `mod inner; pub use inner::*;` puts `inner`'s items on a library's
    // surface without `inner` being `pub`, and the root set follows it
    // ([`SymbolTable::externally_reachable_modules`]). Every case below pins
    // one edge of that closure, or one shape it deliberately leaves alone —
    // and rooting *removes* findings, so the two that must still be reported
    // matter more than the ones that go quiet.

    /// The reproducer from
    /// [#25](https://github.com/rlorenzo/deadwood/issues/25): a consumer can
    /// write `globgap::thing`, so the fact that its only in-workspace referrer
    /// is dead is not evidence about it. `helper`, which nothing names at all,
    /// is right and stays.
    #[test]
    fn an_item_a_pub_use_glob_re_exports_is_a_surface_root() {
        assert_eq!(
            unused_in(&[
                ("", "mod inner;\nmod other;\npub use inner::*;\n"),
                ("inner", "pub fn thing() -> u32 { 1 }\n"),
                (
                    "other",
                    "pub fn helper() -> u32 { crate::inner::thing() }\n"
                ),
            ]),
            vec!["helper"],
            "`thing` is nameable as `fixture::thing`; only `helper` is dead"
        );
    }

    /// The half that must not move, and it is the difference between silencing
    /// a handful of findings and silencing a library's whole surface. A root is
    /// not exempt from condition 1 of [`SymbolTable::unused_definitions`]: an
    /// item behind a glob that *nothing names* is reported exactly as before.
    #[test]
    fn an_item_behind_a_pub_use_glob_that_nothing_names_is_still_reported() {
        assert_eq!(
            unused_in(&[
                ("", "mod inner;\npub use inner::*;\n"),
                ("inner", "pub fn named_by_nobody() -> u32 { 1 }\n"),
            ]),
            vec!["named_by_nobody"],
            "rooting an item never subtracts it from the report"
        );
    }

    /// A glob re-exports the module's `pub` *modules* as well as its
    /// functions, so `facade::nested::deeper` is nameable and the closure has
    /// to descend as well as follow the glob. Stopping at `inner` would leave
    /// the same false positive one level down.
    #[test]
    fn a_pub_module_under_a_glob_re_export_is_a_surface_root_too() {
        assert_eq!(
            unused_in(&[
                ("", "pub mod facade;\nmod other;\n"),
                ("facade", "mod inner;\npub use inner::*;\n"),
                ("facade/inner", "pub mod nested;\n"),
                ("facade/inner/nested", "pub fn deeper() -> u32 { 1 }\n"),
                (
                    "other",
                    "pub fn dead() -> u32 { crate::facade::inner::nested::deeper() }\n"
                ),
            ]),
            vec!["dead"],
            "`deeper` is `facade::nested::deeper` to a consumer"
        );
    }

    /// ...and a *private* module under the same glob is not, because
    /// `pub use inner::*;` re-exports only what is `pub` in `inner`. Without
    /// the `pub` half of the descent the surface would swallow every module
    /// under a glob-exported one.
    #[test]
    fn a_private_module_under_a_glob_re_export_is_not_on_the_surface() {
        assert_eq!(
            unused_in(&[
                ("", "mod inner;\nmod other;\npub use inner::*;\n"),
                ("inner", "mod deep;\n"),
                ("inner/deep", "pub fn buried() -> u32 { 1 }\n"),
                (
                    "other",
                    "pub fn dead() -> u32 { crate::inner::deep::buried() }\n"
                ),
            ]),
            vec!["buried", "dead"],
            "nothing outside can name `deep`, so `buried` falls with its caller"
        );
    }

    /// A plain `use inner::*;` re-exports nothing, so it must not root what it
    /// imports. This is the same claim
    /// `a_private_glob_import_does_not_make_its_source_externally_visible`
    /// makes about the test-only kind, now about the cascade, and without the
    /// `pub` half of the rule every module that imports a glob for its own use
    /// would become public API.
    #[test]
    fn a_private_glob_import_does_not_root_its_source() {
        assert_eq!(
            unused_in(&[
                ("", "mod inner;\nmod other;\nuse inner::*;\n"),
                ("inner", "pub fn thing() -> u32 { 1 }\n"),
                (
                    "other",
                    "pub fn helper() -> u32 { crate::inner::thing() }\n"
                ),
            ]),
            vec!["thing", "helper"],
            "an import is not a re-export, so the cascade still runs"
        );
    }

    /// A binary has no public surface to put anything on, so the glob rule
    /// stops at the crate root and the cascade is untouched. Every case above
    /// would go quiet in a binary too if the closure seeded itself from
    /// anything but a library's crate root.
    #[test]
    fn a_glob_re_export_in_a_binary_puts_nothing_on_a_surface_it_does_not_have() {
        assert_eq!(
            unused_in_binary(&[
                ("", "mod inner;\nmod other;\npub use inner::*;\n"),
                ("inner", "pub fn thing() -> u32 { 1 }\n"),
                (
                    "other",
                    "pub fn helper() -> u32 { crate::inner::thing() }\n"
                ),
            ]),
            vec!["thing", "helper"],
            "nothing outside a binary can name any of it, glob or no glob"
        );
    }

    /// A glob we cannot follow puts nothing on the surface: it is unresolvable,
    /// so it makes its module *opaque* instead, which is already a root in
    /// every walk. Conservatism is unchanged by this phase — an unreadable
    /// mention never becomes evidence, and it never becomes surface either.
    #[test]
    fn a_glob_leading_outside_the_workspace_puts_nothing_on_the_surface() {
        assert_eq!(
            unused_in(&[
                ("", "mod inner;\n"),
                (
                    "inner",
                    "pub use outside::*;\npub fn buried() -> u32 { 1 }\nfn caller() -> u32 { buried() }\n"
                ),
            ]),
            vec!["buried"],
            "`inner` is opaque, not surface, so its own items are judged as before"
        );
    }

    /// A cross-crate glob roots nothing this rule did not already cover. The
    /// only modules of another workspace member a path can name are `pub` from
    /// that member's own crate root, which is the surface already — so the
    /// reading this phase takes ("a consumer *can* name them through this
    /// crate") costs nothing and claims nothing about a crate the glob's author
    /// does not own.
    #[test]
    fn a_cross_crate_glob_re_export_roots_no_module_the_surface_rule_did_not_already_cover() {
        let dep = &[
            ("", "pub mod api;\nmod hidden;\n"),
            ("api", "pub fn open() -> u32 { 1 }\n"),
            (
                "hidden",
                "pub fn buried() -> u32 { 1 }\nfn caller() -> u32 { buried() }\n",
            ),
        ];
        let with_glob = [
            named_unit("dep", dep),
            named_unit("facade", &[("", "pub use dep::*;\n")]),
        ];
        let without = [
            named_unit("dep", dep),
            named_unit("facade", &[("", "pub fn nothing() -> u32 { 1 }\n")]),
        ];

        let mut table = SymbolTable::build(&with_glob);
        let facade_root = *table
            .by_path
            .get(&(1, Vec::new()))
            .expect("the facade crate has a root module");
        assert!(
            !table.modules[facade_root].pub_glob_sources.is_empty(),
            "the glob has to resolve for this test to be about anything"
        );
        table.record_references(&with_glob);
        let reported: Vec<String> = table
            .unused_definitions(&PublicApi::default())
            .into_iter()
            .map(|def| def.name)
            .collect();

        assert_eq!(
            reported,
            vec!["open", "buried"],
            "`hidden` is private to `dep`, so no glob anywhere can name into it"
        );
        let mut control = SymbolTable::build(&without);
        control.record_references(&without);
        assert_eq!(
            reported,
            control
                .unused_definitions(&PublicApi::default())
                .into_iter()
                .map(|def| def.name)
                .filter(|name| name != "nothing")
                .collect::<Vec<_>>(),
            "the glob changed no answer"
        );
    }

    /// `is_worth_reporting` asks the same surface question about a `pub use`,
    /// and it now gets the same answer: a re-export inside a module a glob
    /// exports is reachable from outside, so it is doing its job by existing
    /// and reporting it would be advice to delete public API.
    #[test]
    fn a_pub_use_inside_a_glob_exported_module_is_not_reported() {
        assert!(
            unused_in(&[
                ("", "mod inner;\npub use inner::*;\n"),
                ("inner", "mod deep;\npub use deep::Thing as Renamed;\n"),
                ("inner/deep", "pub struct Thing;\n"),
            ])
            .is_empty(),
            "`fixture::Renamed` is nameable, and the re-export is what names it"
        );
    }

    /// ...and the same re-export with the glob taken away is reported, along
    /// with the definition under it. A stale `pub use` is the cheapest thing in
    /// this tool to delete, so this is the half of the re-export rule worth
    /// guarding.
    #[test]
    fn a_pub_use_no_glob_exports_is_still_reported_with_the_definition_under_it() {
        assert_eq!(
            unused_in(&[
                ("", "mod inner;\nuse inner::*;\n"),
                ("inner", "mod deep;\npub use deep::Thing as Renamed;\n"),
                ("inner/deep", "pub struct Thing;\n"),
            ]),
            vec!["Thing", "Renamed"],
            "outside code cannot reach either, and they are two deletions"
        );
    }

    /// Every referrer of a definition is another way for it to stay alive, so
    /// the edge is recorded even when the definition has been seen already.
    /// Recording only the first would make liveness depend on the order the
    /// walk happens to reach things in.
    #[test]
    fn a_second_referrer_of_an_already_reached_definition_records_its_own_edge() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn shared() -> u32 { 1 }\npub fn dead() -> u32 { shared() }\n#[test]\nfn t() { shared(); }\n"
            ),
            vec!["dead"],
            "the dead referrer comes first in the walk; the live one still counts"
        );
    }

    /// An item nested in a function body is never collected into the symbol
    /// table, so there is no definition to attribute its paths to. Falling
    /// back to a root is the direction that keeps what it names alive.
    #[test]
    fn an_item_nested_in_a_function_body_keeps_what_it_names_alive() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn helper() -> u32 { 1 }\npub fn dead_outer() -> u32 { fn inner() -> u32 { helper() } inner() }\n"
            ),
            vec!["dead_outer"]
        );
    }

    /// A `const` or `static` initializer is written inside its own
    /// definition, so a table of function pointers nothing reaches takes the
    /// functions in it down.
    #[test]
    fn a_const_initializer_belongs_to_the_const() {
        assert_eq!(
            unused_in_binary_root(
                "pub fn handler() -> u32 { 1 }\npub const TABLE: [fn() -> u32; 1] = [handler];\n"
            ),
            vec!["handler", "TABLE"]
        );
    }

    /// A field's type is written inside the struct, so reaching the struct
    /// reaches it and not reaching the struct does not.
    #[test]
    fn a_field_type_belongs_to_the_struct_declaring_it() {
        assert_eq!(
            unused_in_binary_root("pub struct Inner;\npub struct Outer { pub field: Inner }\n"),
            vec!["Inner", "Outer"]
        );
        assert!(
            unused_in_binary_root(
                "pub struct Inner;\npub struct Outer { pub field: Inner }\nfn main() { let _: Outer; }\n"
            )
            .is_empty()
        );
    }

    /// A test harness calls a `#[test]` function that no path names, so it is
    /// a root. The last path segment decides, which is what makes
    /// `#[tokio::test]` and `#[async_std::test]` count too.
    #[test]
    fn a_test_attribute_makes_a_function_a_root() {
        for attribute in ["#[test]", "#[tokio::test]", "#[bench]"] {
            assert!(
                unused_in_binary_root(&format!(
                    "pub fn helper() -> u32 {{ 1 }}\n{attribute}\nfn t() {{ helper(); }}\n"
                ))
                .is_empty(),
                "`{attribute}` is an entry point"
            );
        }
    }

    /// `#[allow(dead_code)]` is the author keeping an item on purpose, so it
    /// is a root and what it names stays alive. `#[allow(unused_variables)]`
    /// is about a parameter and says nothing about whether the item is
    /// reached — rooting on it would silence a whole cascade under one of the
    /// most common attributes in Rust, so the lint name has to match whole
    /// rather than as a substring.
    #[test]
    fn only_the_dead_code_opt_outs_root_an_item() {
        for attribute in [
            "#[allow(dead_code)]",
            "#[expect(dead_code)]",
            "#[allow(unused)]",
        ] {
            assert!(
                unused_in_binary_root(&format!(
                    "pub fn helper() -> u32 {{ 1 }}\n{attribute}\npub fn kept() -> u32 {{ helper() }}\n"
                ))
                .is_empty(),
                "`{attribute}` keeps the item, and so what it names"
            );
        }
        for attribute in [
            "#[allow(unused_variables)]",
            "#[allow(unused_mut)]",
            "#[expect(unused_imports)]",
        ] {
            // The item itself is still suppressed — that is `has_skip_attr`,
            // unchanged, and the author has said they do not want to hear
            // about it — but it is not reached, so what it names falls.
            assert_eq!(
                unused_in_binary_root(&format!(
                    "pub fn helper() -> u32 {{ 1 }}\n{attribute}\npub fn caller(spare: u32) -> u32 {{ helper() }}\n"
                )),
                vec!["helper"],
                "`{attribute}` silences its own item without rooting the cascade"
            );
        }
    }

    /// Edition 2024 spells the linker exports `#[unsafe(no_mangle)]`, which
    /// parses as an attribute named `unsafe` holding the real one. Reading the
    /// outer path alone would report every modern export, and everything it
    /// calls, as dead.
    #[test]
    fn an_unsafe_wrapped_export_is_still_an_export() {
        assert!(
            unused_in_binary_root(
                "pub fn helper() -> u32 { 1 }\n#[unsafe(no_mangle)]\npub extern \"C\" fn exported() -> u32 { helper() }\n"
            )
            .is_empty()
        );
        assert!(
            unused_in_binary_root(
                "pub fn helper() -> u32 { 1 }\n#[unsafe(export_name = \"x\")]\npub fn exported() -> u32 { helper() }\n"
            )
            .is_empty()
        );
    }

    /// A macro name lives in its own namespace, so neither a generic
    /// parameter nor a local shadows it — but a macro reached from inside a
    /// path's generic arguments inherits that path's namespace.
    #[test]
    fn a_macro_name_is_not_shadowed_by_a_binding_of_the_same_name() {
        assert_eq!(
            unused_in_root(
                "pub fn thing() -> u32 { 1 }\npub fn caller<thing>(_v: Vec<thing!()>) -> u32 { 0 }\n"
            ),
            vec!["caller"]
        );
        assert_eq!(
            unused_in_root(
                "pub fn thing() -> u32 { 1 }\npub fn caller() -> u32 { let thing = 2; take::<{ thing!() }>() }\n"
            ),
            vec!["caller"]
        );
    }

    // -- test-only items ---------------------------------------------------
    //
    // Every case below is a claim about the *difference* between two walks
    // over one edge set, so each pins one clause of `is_root` under
    // `RootSet::WithoutTests`. Inverting that clause is what has to turn the
    // assertion red.

    /// The claim itself: a `pub` item a private module holds, referenced from
    /// a `#[test]` function and nowhere else, is reached — so it is not an
    /// unused-pub finding — and is reached only by dropping the test roots.
    #[test]
    fn an_item_only_a_test_function_reaches_is_test_only() {
        let sources = &[
            (
                "",
                "mod inner;\n#[test]\nfn covered() { inner::helper(); }\n",
            ),
            ("inner", "pub fn helper() {}\n"),
        ];
        assert_eq!(test_only_in_library(sources), vec!["helper"]);
        // ...and it is *not* also an unused-pub finding: the two lists
        // describe different deletions and must never describe one item.
        assert!(
            unused_in(sources).is_empty(),
            "a test-only item is referenced and reached, so nothing calls it dead"
        );
    }

    /// The other half of that claim. `fn main` is an entry point of every
    /// build, so an item it reaches is reached without the tests too.
    #[test]
    fn an_item_a_non_test_entry_point_also_reaches_is_not_test_only() {
        assert!(
            test_only_in_library(&[
                (
                    "",
                    "mod inner;\nfn main() { inner::helper(); }\n\
                     #[test]\nfn covered() { inner::helper(); }\n",
                ),
                ("inner", "pub fn helper() {}\n"),
            ])
            .is_empty()
        );
    }

    /// A test, bench or example target is test code in its entirety, so an
    /// entry point in one — a `harness = false` test's `fn main`, a bench
    /// runner — is a test root exactly as a `#[test]` function is. Without
    /// the target half of the split this `fn main` would root `helper` in
    /// both walks and the finding would vanish.
    #[test]
    fn an_entry_point_in_a_test_target_is_a_test_root_by_the_target_alone() {
        let target = CrateUnit {
            names: Vec::new(),
            test_code: true,
            files: unit(&[("", "mod support;\nfn main() { support::from_target(); }\n")])
                .files
                .into_iter()
                .chain(unit(&[("support", "pub fn from_target() {}\n")]).files)
                .collect(),
        };
        assert_eq!(test_only_in(&[target]), vec!["from_target"]);
    }

    /// The same `fn main` in an ordinary target roots what it reaches in both
    /// walks. This is the assertion the case above is measured against: the
    /// only difference between them is `CrateUnit::test_code`.
    #[test]
    fn an_entry_point_in_an_ordinary_target_is_not_a_test_root() {
        let target = CrateUnit {
            names: Vec::new(),
            test_code: false,
            files: unit(&[("", "mod support;\nfn main() { support::from_target(); }\n")])
                .files
                .into_iter()
                .chain(unit(&[("support", "pub fn from_target() {}\n")]).files)
                .collect(),
        };
        assert!(test_only_in(&[target]).is_empty());
    }

    /// A file only `#[cfg(test)] mod` declarations reach is test code however
    /// ordinary its own attributes look (phase 7's flag), so an entry point
    /// written in one — here the `dead_code` opt-out — is a test root too.
    #[test]
    fn an_entry_point_in_a_test_only_file_is_a_test_root() {
        let mut crate_unit = unit(&[
            ("", "mod inner;\nmod tests;\n"),
            ("inner", "pub fn helper() {}\n"),
            (
                "tests",
                "#[allow(dead_code)]\nfn kept() { crate::inner::helper(); }\n",
            ),
        ]);
        crate_unit.files[2].test_only = true;
        assert_eq!(test_only_in(&[crate_unit]), vec!["helper"]);
    }

    /// An opaque mention is a root in *both* walks, so an item one names can
    /// never be test-only however test-only it looks. This is the conservative
    /// direction and it is expensive: an `assert_eq!` naming the item is
    /// enough, which is most of the recall this kind gives up.
    #[test]
    fn an_opaque_mention_keeps_an_item_out_of_the_kind() {
        assert!(
            test_only_in_library(&[
                (
                    "",
                    "mod inner;\n#[test]\nfn covered() { assert_eq!(inner::helper(), 1); }\n",
                ),
                ("inner", "pub fn helper() -> u32 { 1 }\n"),
            ])
            .is_empty()
        );
    }

    /// A library's public surface is a root in both walks: consumers Deadwood
    /// cannot see reach it in a build with no tests at all, so "only tests
    /// reach this" is not a claim we are entitled to make about it.
    #[test]
    fn a_librarys_public_surface_is_never_test_only() {
        assert!(
            test_only_in_library(&[
                (
                    "",
                    "pub mod exposed;\n#[test]\nfn covered() { exposed::helper(); }\n"
                ),
                ("exposed", "pub fn helper() {}\n"),
            ])
            .is_empty()
        );
    }

    /// `mod inner; pub use inner::*;` puts a private module's items on the
    /// surface without making the module `pub`, and a glob binds no name, so
    /// the root set does not see it. `winnow`'s `combinator::iterator` is that
    /// shape: documented public API whose only in-crate callers are tests.
    #[test]
    fn an_item_a_pub_use_glob_re_exports_is_never_test_only() {
        assert!(
            test_only_in_library(&[
                ("", "pub mod facade;\n"),
                (
                    "facade",
                    "mod inner;\npub use inner::*;\n#[test]\nfn covered() { helper(); }\n",
                ),
                ("facade/inner", "pub fn helper() {}\n"),
            ])
            .is_empty()
        );
    }

    /// A glob re-export carries the module's `pub` children with it:
    /// `pub use inner::*;` makes `inner::nested` nameable as `facade::nested`,
    /// so an item in it is surface for the same reason `from_glob` is. The
    /// closure has to descend as well as follow globs, or it stops one level
    /// short of the API.
    #[test]
    fn a_pub_module_under_a_glob_re_export_is_never_test_only() {
        assert!(
            test_only_in_library(&[
                ("", "pub mod facade;\n"),
                (
                    "facade",
                    "mod inner;\npub use inner::*;\n#[test]\nfn covered() { inner::nested::deeper(); }\n",
                ),
                ("facade/inner", "pub mod nested;\n"),
                ("facade/inner/nested", "pub fn deeper() {}\n"),
            ])
            .is_empty()
        );
    }

    /// ...and a plain `use inner::*;` re-exports nothing, so it must not buy
    /// the same silence. Without the `pub` half of that rule this case would
    /// go quiet too, and the kind would be silent wherever a module imports a
    /// glob for its own use.
    #[test]
    fn a_private_glob_import_does_not_make_its_source_externally_visible() {
        assert_eq!(
            test_only_in_library(&[
                ("", "pub mod facade;\n"),
                (
                    "facade",
                    "mod inner;\nuse inner::*;\n#[test]\nfn covered() { helper(); }\n",
                ),
                ("facade/inner", "pub fn helper() {}\n"),
            ]),
            vec!["helper"]
        );
    }

    /// An item nothing names at all is dead, which is the stronger claim and
    /// already reported. Saying "only tests reach this" about it as well would
    /// be two findings for one deletion.
    #[test]
    fn an_item_nothing_names_is_an_unused_finding_and_not_a_test_only_one() {
        let sources = &[
            ("", "mod inner;\n#[test]\nfn covered() {}\n"),
            ("inner", "pub fn orphan() {}\n"),
        ];
        assert_eq!(unused_in(sources), vec!["orphan"]);
        assert!(test_only_in_library(sources).is_empty());
    }

    /// An item only a *dead* test helper reaches is dead, not test-only: the
    /// second walk is not the only one that has to reach it.
    #[test]
    fn an_item_only_unreached_test_code_names_is_an_unused_finding() {
        let sources = &[
            ("", "mod inner;\n"),
            (
                "inner",
                "pub fn helper() {}\nfn never_run() { helper(); }\n",
            ),
        ];
        assert_eq!(unused_in(sources), vec!["helper"]);
        assert!(test_only_in_library(sources).is_empty());
    }

    /// The surface is a root in the second walk, not merely excluded from the
    /// report: what a surface item *reaches* is reached without the tests too.
    /// `helper` here is `pub` in a private module — the shape this kind exists
    /// for — and it is quiet because the one thing naming it is on the
    /// surface, which a consumer we cannot see can call.
    #[test]
    fn an_item_a_surface_item_reaches_is_not_test_only() {
        assert!(
            test_only_in_library(&[
                ("", "pub mod exposed;\nmod inner;\n"),
                (
                    "exposed",
                    "pub fn entry() -> u32 { crate::inner::helper() }\n"
                ),
                ("inner", "pub fn helper() -> u32 { 1 }\n"),
            ])
            .is_empty()
        );
    }

    /// A root nothing names is an unused finding — the stronger claim, and
    /// already reported — so it must not be a test-only finding as well. Only
    /// items something *names* can reach this kind at all.
    #[test]
    fn a_test_root_nothing_names_is_one_finding_and_not_two() {
        let sources = &[
            ("", "mod inner;\n"),
            ("inner", "#[test]\npub fn covered() {}\n"),
        ];
        assert_eq!(unused_in(sources), vec!["covered"]);
        assert!(
            test_only_in_library(sources).is_empty(),
            "an item nothing names is dead, not test-only"
        );
    }

    /// A definition that is not `pub` is nobody's visibility mistake, and the
    /// advice this kind gives — narrow it — has nothing to say about one.
    #[test]
    fn a_private_item_is_not_reported_as_test_only() {
        assert!(
            test_only_in_library(&[
                (
                    "",
                    "mod inner;\n#[test]\nfn covered() { inner::helper(); }\n"
                ),
                ("inner", "pub(crate) fn helper() {}\n"),
            ])
            .is_empty()
        );
    }
}
