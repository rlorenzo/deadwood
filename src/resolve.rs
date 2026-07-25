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
//! A path that resolves cleanly to nothing in the workspace (a local
//! binding, a generic parameter, `std`, an external crate) marks nothing.
//!
//! # Known limitations
//!
//! - Purely syntactic: method calls (`x.foo()`), trait dispatch, and
//!   associated items are not resolved. Only free-standing item definitions
//!   are reported, so this costs findings, never precision.
//! - `cfg` is not evaluated, so `#[cfg(test)]` code counts as a use.
//! - Edition 2015 crate-relative `use` paths are supported by falling back to
//!   the crate root; other 2015-only path forms may resolve to nothing (which
//!   only ever hides findings).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;

use crate::modtree::ParsedFile;

/// How deep a chain of `use` aliases is followed before giving up and falling
/// back to the conservative rule. Real code never comes close.
const MAX_ALIAS_DEPTH: usize = 8;

/// One compilation unit: a crate root plus every file reachable from it.
pub(crate) struct CrateUnit {
    /// Names other crates can use to refer to this one in paths. Empty for
    /// targets nothing can name (bins, tests, examples, benches).
    pub names: Vec<String>,
    pub files: Vec<ParsedFile>,
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
    /// `use prefix::*` prefixes written in this module, before resolution.
    globs: Vec<RefPath>,
    /// Modules whose names this module pulls in through a resolved glob.
    glob_sources: Vec<usize>,
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
    /// Every definition sharing a name, for the conservative fallback.
    by_name: HashMap<String, Vec<usize>>,
    /// Module lookup by crate and module path.
    by_path: HashMap<(usize, Vec<String>), usize>,
    /// Whether each crate is a library, i.e. whether anything outside the
    /// workspace could name its public items at all.
    is_library: Vec<bool>,
    used: HashSet<usize>,
}

/// A reportable definition that no resolved path reaches.
pub(crate) struct UnusedDef {
    pub name: String,
    pub kind: DefKind,
    pub file: PathBuf,
    pub line: usize,
}

impl SymbolTable {
    /// Index every definition, alias, and glob import in `crates`.
    pub(crate) fn build(crates: &[CrateUnit]) -> Self {
        let mut table = SymbolTable {
            defs: Vec::new(),
            modules: Vec::new(),
            roots: Vec::new(),
            externs: HashMap::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            is_library: crates.iter().map(|unit| !unit.names.is_empty()).collect(),
            used: HashSet::new(),
        };

        for krate in 0..crates.len() {
            let root = table.new_module(krate, None, Vec::new());
            table.roots.push(root);
        }
        // A target's own library name is authoritative; the package name is
        // only a fallback for the common case where the two agree.
        for (krate, unit) in crates.iter().enumerate() {
            if let Some(name) = unit.names.first() {
                table.externs.insert(name.clone(), table.roots[krate]);
            }
        }
        for (krate, unit) in crates.iter().enumerate() {
            for name in unit.names.iter().skip(1) {
                table
                    .externs
                    .entry(name.clone())
                    .or_insert(table.roots[krate]);
            }
        }

        for (krate, unit) in crates.iter().enumerate() {
            for file in &unit.files {
                let Some(ast) = &file.ast else { continue };
                let module = table.module_for(krate, &file.module);
                table.collect_items(&ast.items, module, file, true);
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
                };
                for item in &ast.items {
                    walker.visit_item(item);
                }
            }
        }
    }

    /// Reportable definitions that no resolved path reaches, ordered by
    /// source location.
    ///
    /// Definitions are deduplicated by location: a file pulled into several
    /// crates (via `#[path]`) defines the same item once per crate, and it
    /// counts as used if *any* of those crates uses it.
    pub(crate) fn unused_definitions(&self) -> Vec<UnusedDef> {
        let site = |def: &Def| (def.file.clone(), def.line, def.name.clone());
        let used: HashSet<_> = self.used.iter().map(|&id| site(&self.defs[id])).collect();

        let mut seen = HashSet::new();
        let mut out: Vec<UnusedDef> = self
            .defs
            .iter()
            .filter(|def| def.reportable && self.is_worth_reporting(def))
            .filter(|def| !used.contains(&site(def)) && seen.insert(site(def)))
            .map(|def| UnusedDef {
                name: def.name.clone(),
                kind: def.kind,
                file: def.file.clone(),
                line: def.line,
            })
            .collect();
        out.sort_by(|a, b| (&a.file, a.line, &a.name).cmp(&(&b.file, b.line, &b.name)));
        out
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
    fn is_worth_reporting(&self, def: &Def) -> bool {
        !def.kind.is_reexport() || !self.is_externally_reachable(def.module)
    }

    /// Whether code outside the workspace could name items in this module:
    /// it belongs to a library, and every `mod` on the way in is `pub`.
    fn is_externally_reachable(&self, module: usize) -> bool {
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
        self.defs.push(def);
        id
    }

    /// Record the module-level items of `items` into `module`.
    ///
    /// `top_level` is false inside a function body, where a `pub use` binds a
    /// name locally but re-exports nothing.
    fn collect_items(
        &mut self,
        items: &[syn::Item],
        module: usize,
        file: &ParsedFile,
        top_level: bool,
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
                        child: Some(child),
                        target: None,
                    });
                    if let Some((_, inner)) = &m.content {
                        self.collect_items(inner, child, file, true);
                    }
                }
                syn::Item::Use(u) => self.add_use(u, module, file, top_level),
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
                            child: Some(root),
                            target: None,
                        });
                    }
                }
                other => {
                    if let Some((ident, kind, attrs, vis)) = describe(other) {
                        let name = ident.to_string();
                        let reportable = matches!(vis, syn::Visibility::Public(_))
                            && !(kind == DefKind::Fn && name == "main")
                            && !has_skip_attr(attrs);
                        self.add_def(Def {
                            name,
                            kind,
                            file: file.path.clone(),
                            line: ident.span().start().line,
                            module,
                            reportable,
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
                    };
                    nested.visit_item(other);
                }
            }
        }
    }

    fn add_use(&mut self, item: &syn::ItemUse, module: usize, file: &ParsedFile, top_level: bool) {
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
                self.modules[module].globs.push(path);
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
        let mut pending: Vec<(usize, RefPath)> = self
            .modules
            .iter()
            .enumerate()
            .flat_map(|(id, m)| m.globs.iter().cloned().map(move |glob| (id, glob)))
            .collect();

        loop {
            let before = pending.len();
            let mut still_pending = Vec::new();
            for (module, glob) in pending {
                match self.walk_path(module, &glob, true, 0, &mut Vec::new()) {
                    Outcome::Module(target) => self.modules[module].glob_sources.push(target),
                    _ => still_pending.push((module, glob)),
                }
            }
            if still_pending.len() == before {
                for (module, _) in still_pending {
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
    /// `reached`.
    ///
    /// `in_use` marks paths written in a `use` declaration, which may be
    /// crate-root-relative in edition 2015.
    fn walk_path(
        &self,
        module: usize,
        path: &RefPath,
        in_use: bool,
        depth: usize,
        reached: &mut Vec<usize>,
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
                    reached.extend(&defs);
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

    /// Mark everything `path`, written in `module`, refers to.
    fn mark_path_used(&mut self, module: usize, path: &RefPath, in_use: bool) {
        let mut reached = Vec::new();
        let outcome = self.walk_path(module, path, in_use, 0, &mut reached);
        for id in reached {
            self.mark_def_used(id);
        }
        if let Outcome::Opaque(from) = outcome {
            for index in from..path.segments.len() {
                let name = path.segments[index].clone();
                self.mark_name_used(&name);
            }
        }
    }

    fn mark_def_used(&mut self, id: usize) {
        if !self.used.insert(id) {
            // Already marked; this also stops cycles of mutual re-exports.
            return;
        }
        // Reaching an alias reaches whatever it re-exports.
        if let Some(target) = self.defs[id].target.clone() {
            let module = self.defs[id].module;
            self.mark_path_used(module, &target, true);
        }
    }

    /// The conservative fallback: treat every definition with this name,
    /// anywhere in the workspace, as used.
    fn mark_name_used(&mut self, name: &str) {
        let Some(ids) = self.by_name.get(name) else {
            return;
        };
        for id in ids.clone() {
            self.mark_def_used(id);
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
                self.mark_def_used(id);
            }
        }
    }

    fn mark_path_names_used(&mut self, path: &syn::Path) {
        for segment in &path.segments {
            self.mark_name_used(&segment.ident.to_string());
        }
    }
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
}

impl<'ast> Visit<'ast> for RefWalker<'_> {
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
        // A `use` is a reference to whatever it imports — including a
        // `pub use`, so a dead re-export is reported once, as a re-export,
        // instead of cascading into a second finding about its target.
        let absolute = node.leading_colon.is_some();
        let mut leaves = Vec::new();
        flatten_use(&node.tree, &mut Vec::new(), &mut leaves);
        for leaf in leaves {
            let path = RefPath {
                absolute,
                segments: leaf.segments,
            };
            self.table.mark_path_used(self.module, &path, true);
        }
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let path = RefPath::from_syn(node);
        // `Foo::new()` or `Foo { .. }` written inside `impl Foo` says nothing
        // about whether anyone uses `Foo`; it is the same self-reference as
        // the `impl` header itself.
        let names_self = node.leading_colon.is_none()
            && self.impl_self.as_deref() == path.segments.first().map(String::as_str);
        if !names_self {
            self.table.mark_path_used(self.module, &path, false);
        }
        // Generic arguments inside the segments are paths of their own.
        syn::visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        syn::visit::visit_macro(self, node);
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
                // Only a bare `impl Foo` gives the body a self-reference to
                // recognize. For `impl crate::foo::Bar`, the head segment is
                // a qualifier, and suppressing every `crate::` path in the
                // body would hide real uses.
                if ty.path.leading_colon.is_none() && ty.path.segments.len() == 1 {
                    self_name = Some(ty.path.segments[0].ident.to_string());
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
}

impl<'ast> Visit<'ast> for NestedUses<'_> {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.table.add_use(node, self.module, self.file, false);
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

/// Attributes that mark an item as used externally or deliberately kept.
fn has_skip_attr(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        if path.is_ident("no_mangle") || path.is_ident("used") || path.is_ident("export_name") {
            return true;
        }
        if (path.is_ident("allow") || path.is_ident("expect"))
            && let syn::Meta::List(list) = &attr.meta
        {
            let lints = list.tokens.to_string();
            return lints.contains("dead_code") || lints.contains("unused");
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crate whose files are `(module path, source)`; an empty module path
    /// is the crate root.
    fn unit(sources: &[(&str, &str)]) -> CrateUnit {
        CrateUnit {
            names: vec!["fixture".to_string()],
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
                })
                .collect(),
        }
    }

    fn module_at(table: &SymbolTable, path: &[&str]) -> usize {
        let path: Vec<String> = path.iter().map(|s| (*s).to_string()).collect();
        *table
            .by_path
            .get(&(0, path.clone()))
            .unwrap_or_else(|| panic!("no module at {path:?}"))
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
}
