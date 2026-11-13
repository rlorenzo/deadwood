//! What a proc macro's expansion may name, read from the macro's source.
//!
//! A derive's output names crates the deriving package never writes: ruff's
//! `#[derive(CacheKey)]` expands to `ruff_cache::CacheKey`, and bevy's
//! `#[derive(ExtractComponent)]` looks `bevy_extract` up in the consumer's
//! manifest. The expansion cannot be computed without running the macro, but
//! its source says what it can write: the identifiers its `quote!`-family
//! invocations use as path heads, and the words of its path-like strings.
//!
//! Those words are kept per function, with the functions each one calls by
//! path (`helper(x)`, `Self::emit(..)`, `utils::get_path(..)`), so
//! that a package is credited with what the macros it *uses* write and not
//! with everything the macro crate can write. `bevy_render_macros` defines
//! both `AsBindGroup`, which names `bevy_image`, and `SpecializerKey`, which
//! does not; crediting the whole crate hid a stale `bevy_image` entry in a
//! package that only derives `SpecializerKey`. Calls are matched by name
//! alone, so two helpers sharing one are both followed, which only ever
//! credits more. Method calls are not followed: `.parse()`, `.into()` and
//! `.new()` name a trait method of every type in the crate, and following
//! them joined every derive to every other.

use std::collections::{HashMap, HashSet};

use proc_macro2::{Spacing, TokenStream, TokenTree};
use syn::visit::Visit;

use crate::resolve::CrateUnit;
use crate::{cfg, config, deps, metadata, modtree};

/// Each package's dependency references, as `analyze_with` collects them.
type References<'m, 'g> = [(&'m metadata::Package, deps::CrateReferences, cfg::Gates<'g>)];

/// Count what other crates' macros write as mentions by the packages that
/// invoke them: the path heads of a workspace `macro_rules!` body, and what a
/// proc macro and its helper libraries can expand to, from the workspace or
/// from the registry cache.
pub(crate) fn credit_macros(
    meta: &metadata::Metadata,
    config: &config::Config,
    crates: &[CrateUnit],
    references: &mut References<'_, '_>,
) {
    let (bodies, defined_by) = macro_bodies(crates);
    let none = HashSet::new();
    for (package, package_references, _) in references.iter_mut() {
        let own = defined_by.get(package.name.as_str()).unwrap_or(&none);
        package_references.add_foreign_macros(&bodies, own);
    }
    workspace_proc_macros(meta, crates, references);
    external_proc_macros(meta, config, references);
}

/// What a workspace proc macro, read with its workspace helper libraries,
/// writes for the packages that use it.
///
/// A package uses one when its code mentions one of the macro's entry points
/// and the proc-macro package is among its workspace dependencies, however
/// indirectly — a derive is as often reached through a library's re-export
/// (bevy's `ExtractComponent` through `bevy_render`) as depended on directly.
/// The helpers are the workspace packages the macro depends on, however
/// indirectly, that write code themselves ([`metadata::Package::writes_code`]):
/// `bevy_macro_utils` builds the paths bevy's derives emit.
fn workspace_proc_macros(
    meta: &metadata::Metadata,
    crates: &[CrateUnit],
    references: &mut References<'_, '_>,
) {
    // Each package's crates, read once however many macros share a helper.
    let mut libs: HashMap<&str, MacroSource> = HashMap::new();
    let mut macros: HashMap<&str, MacroSource> = HashMap::new();
    for unit in crates {
        let package = unit.package.as_str();
        // A library is a crate something can name; tests and bins are not.
        let into = if unit.is_proc_macro {
            macros.entry(package).or_default()
        } else if !unit.names.is_empty() {
            libs.entry(package).or_default()
        } else {
            continue;
        };
        into.read_files(unit.files.iter().chain(&unit.spliced));
    }
    for (&proc_macro, source) in &mut macros {
        for helper in meta.workspace_reach([proc_macro]) {
            if meta
                .member(helper)
                .is_some_and(metadata::Package::writes_code)
                && let Some(lib) = libs.get(helper)
            {
                source.absorb(lib);
            }
        }
    }
    if macros.is_empty() {
        return;
    }
    for (package, package_references, _) in references.iter_mut() {
        let reached = meta.workspace_reach([package.name.as_str()]);
        let used: Vec<&MacroSource> = macros
            .iter()
            .filter(|(proc_macro, _)| reached.contains(*proc_macro))
            .map(|(_, source)| source)
            .collect();
        credit_all(&used, package_references);
    }
}

/// Credit `sources` to a package, deciding which of their macros it uses
/// from its own code before adding any of their output. Crediting one source
/// at a time let a word one added mark another's entry point as mentioned, so
/// the answer depended on the order the sources came in; and settling that
/// to a fixed point instead let helper-attribute names as plain as `texture`
/// chain one derive's output into every other's.
fn credit_all(sources: &[&MacroSource], references: &mut deps::CrateReferences) {
    let words: Vec<HashSet<String>> = sources
        .iter()
        .map(|source| {
            source.words_for(
                source
                    .entry_names()
                    .filter(|name| references.mentions(name)),
            )
        })
        .collect();
    for words in &words {
        references.add_opaque_words(words);
    }
}

/// [`workspace_proc_macros`] for proc macros from outside the workspace, read
/// from the source the full resolution points at: see
/// [`metadata::ExternalMacro`].
///
/// Each source is resolved from its lib roots like a workspace crate, under
/// the configured `cfg` matrix and with none of the project's `ignore`
/// patterns, once however many macros share it, and anything it warns about
/// is dropped: a file that cannot be read there costs a mention, never a
/// finding.
fn external_proc_macros(
    meta: &metadata::Metadata,
    config: &config::Config,
    references: &mut References<'_, '_>,
) {
    let unconfigured = config::Config::default();
    let mut read: HashMap<&std::path::Path, MacroSource> = HashMap::new();
    let mut sources: Vec<(&HashSet<String>, MacroSource)> = Vec::new();
    for external in &meta.external_macros {
        if !references
            .iter()
            .any(|(package, _, _)| external.users.contains(&package.name))
        {
            continue;
        }
        let mut source = MacroSource::default();
        for package in &external.sources {
            let one = read.entry(&package.manifest_path).or_insert_with(|| {
                let gates = cfg::Gates::new(config.cfg(), package);
                let mut one = MacroSource::default();
                for target in package.targets.iter().filter(|target| target.is_lib()) {
                    let mut ignored = Vec::new();
                    let resolved = modtree::resolve(
                        &target.src_path,
                        unconfigured.ignore(),
                        &gates,
                        &mut ignored,
                    );
                    one.read_files(resolved.files.iter().chain(&resolved.spliced));
                }
                one
            });
            source.absorb(one);
        }
        sources.push((&external.users, source));
    }
    for (package, package_references, _) in references.iter_mut() {
        let used: Vec<&MacroSource> = sources
            .iter()
            .filter(|(users, _)| users.contains(&package.name))
            .map(|(_, source)| source)
            .collect();
        credit_all(&used, package_references);
    }
}

/// Words by name: a macro's body words, or a package's defined macro names.
type WordSets<K> = HashMap<K, HashSet<String>>;

/// The path heads of every `macro_rules!` body in the workspace, by macro
/// name, with the bodies of the macros each invokes folded in — and the names
/// each package defines.
fn macro_bodies(crates: &[CrateUnit]) -> (WordSets<String>, WordSets<&str>) {
    fn items<'a>(found: &mut Vec<&'a syn::ItemMacro>, list: &'a [syn::Item]) {
        for item in list {
            match item {
                syn::Item::Macro(mac) if mac.mac.path.is_ident("macro_rules") => found.push(mac),
                syn::Item::Mod(module) => {
                    if let Some((_, inner)) = &module.content {
                        items(found, inner);
                    }
                }
                _ => {}
            }
        }
    }

    let mut direct: HashMap<String, (HashSet<String>, HashSet<String>)> = HashMap::new();
    let mut defined_by: HashMap<&str, HashSet<String>> = HashMap::new();
    for unit in crates {
        let package = unit.package.as_str();
        for file in unit.files.iter().chain(&unit.spliced) {
            let Some(ast) = &file.ast else { continue };
            let mut found = Vec::new();
            items(&mut found, &ast.items);
            for mac in found {
                let Some(name) = &mac.ident else { continue };
                let name = name.to_string();
                let (heads, invokes) = direct.entry(name.clone()).or_default();
                path_heads(&mac.mac.tokens, heads);
                invocations(&mac.mac.tokens, invokes);
                defined_by.entry(package).or_default().insert(name);
            }
        }
    }
    let bodies = direct
        .keys()
        .map(|name| {
            let mut all = HashSet::new();
            let mut seen = HashSet::new();
            let mut pending = vec![name.as_str()];
            while let Some(next) = pending.pop() {
                if !seen.insert(next) {
                    continue;
                }
                if let Some((body, invokes)) = direct.get(next) {
                    all.extend(body.iter().cloned());
                    pending.extend(invokes.iter().map(String::as_str));
                }
            }
            (name.clone(), all)
        })
        .collect();
    (bodies, defined_by)
}

/// Whether `next`, the token after an identifier, makes it a macro
/// invocation: `name!`.
pub(crate) fn bang_follows(next: Option<&TokenTree>) -> bool {
    matches!(next, Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
}

/// Every `name!` invocation in `tokens`, at any depth.
fn invocations(tokens: &TokenStream, into: &mut HashSet<String>) {
    let mut iter = tokens.clone().into_iter().peekable();
    while let Some(tree) = iter.next() {
        match tree {
            TokenTree::Ident(ident) if bang_follows(iter.peek()) => {
                into.insert(ident.to_string());
            }
            TokenTree::Group(group) => invocations(&group.stream(), into),
            _ => {}
        }
    }
}

/// The words of `text`: its runs of letters, digits and underscores.
pub(crate) fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
}

/// One proc-macro crate, or several read as one: an external macro's helper
/// libraries (`tauri-codegen`, `tauri-utils`) write its output as much as the
/// macro crate does.
#[derive(Default)]
pub(crate) struct MacroSource {
    /// The names a consumer writes — a derive, its helper attributes, an
    /// attribute or function-like macro — to the functions implementing them.
    entries: HashMap<String, HashSet<String>>,
    /// What each function writes, by function name.
    words: HashMap<String, HashSet<String>>,
    /// The functions each function calls, by name.
    calls: HashMap<String, HashSet<String>>,
    /// What code outside any function writes and calls — a `const`, a
    /// `static` — counted for every entry, since any of them may use it.
    global_words: HashSet<String>,
    global_calls: HashSet<String>,
    /// The functions being walked, innermost last.
    current: Vec<String>,
}

impl MacroSource {
    /// Read one file of the crate.
    pub(crate) fn read(&mut self, file: &syn::File) {
        self.visit_file(file);
    }

    /// Every name a consumer can write to use one of the crate's macros.
    pub(crate) fn entry_names(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    fn read_files<'f>(&mut self, files: impl Iterator<Item = &'f modtree::ParsedFile>) {
        for file in files {
            if let Some(ast) = &file.ast {
                self.read(ast);
            }
        }
    }

    /// Add `other`'s macros and functions to this source: a macro calls into
    /// its helper libraries by path, so they are read as one.
    fn absorb(&mut self, other: &MacroSource) {
        fn merge(
            into: &mut HashMap<String, HashSet<String>>,
            from: &HashMap<String, HashSet<String>>,
        ) {
            for (key, values) in from {
                into.entry(key.clone())
                    .or_default()
                    .extend(values.iter().cloned());
            }
        }
        merge(&mut self.entries, &other.entries);
        merge(&mut self.words, &other.words);
        merge(&mut self.calls, &other.calls);
        self.global_words.extend(other.global_words.iter().cloned());
        self.global_calls.extend(other.global_calls.iter().cloned());
    }

    /// What using the macros named `used` may expand to: the words of the
    /// functions implementing them, of every function they call, however
    /// indirectly, and of the code outside any function. Nothing when no
    /// macro is used.
    pub(crate) fn words_for<'a>(&self, used: impl Iterator<Item = &'a String>) -> HashSet<String> {
        let mut pending: Vec<&str> = used
            .filter_map(|name| self.entries.get(name))
            .flatten()
            .map(String::as_str)
            .collect();
        if pending.is_empty() {
            return HashSet::new();
        }
        pending.extend(self.global_calls.iter().map(String::as_str));
        let mut out = self.global_words.clone();
        let mut seen = HashSet::new();
        while let Some(function) = pending.pop() {
            if !seen.insert(function) {
                continue;
            }
            if let Some(words) = self.words.get(function) {
                out.extend(words.iter().cloned());
            }
            if let Some(calls) = self.calls.get(function) {
                pending.extend(calls.iter().map(String::as_str));
            }
        }
        out
    }

    fn sink(&mut self) -> &mut HashSet<String> {
        match self.current.last() {
            Some(function) => self.words.entry(function.clone()).or_default(),
            None => &mut self.global_words,
        }
    }

    fn call(&mut self, name: String) {
        match self.current.last() {
            Some(function) => {
                self.calls.entry(function.clone()).or_default().insert(name);
            }
            None => {
                self.global_calls.insert(name);
            }
        }
    }

    fn within(&mut self, function: String, body: impl FnOnce(&mut Self)) {
        self.current.push(function);
        body(self);
        self.current.pop();
    }
}

impl<'ast> Visit<'ast> for MacroSource {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let function = node.sig.ident.to_string();
        for attr in &node.attrs {
            let path = attr.path();
            let mut names = HashSet::new();
            if path.is_ident("proc_macro") || path.is_ident("proc_macro_attribute") {
                names.insert(function.clone());
            } else if path.is_ident("proc_macro_derive")
                && let syn::Meta::List(list) = &attr.meta
            {
                // The derive's name and its `attributes(..)` helpers.
                idents(&list.tokens, &mut names);
                names.remove("attributes");
            }
            for name in names {
                self.entries
                    .entry(name)
                    .or_default()
                    .insert(function.clone());
            }
        }
        self.within(function, |walker| syn::visit::visit_item_fn(walker, node));
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let function = node.sig.ident.to_string();
        self.within(function, |walker| {
            syn::visit::visit_impl_item_fn(walker, node);
        });
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        syn::visit::visit_macro(self, node);
        let quoting = node
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident.to_string().contains("quote"));
        let mut found = HashSet::new();
        // `quote!`, `quote_spanned!`, `parse_quote!` and the like write paths;
        // `format!("{}::Key", krate)` and friends build them from strings.
        if quoting {
            path_heads(&node.tokens, &mut found);
        }
        literals(&node.tokens, &mut found);
        // Helpers are called from macro input too: `quote! { #(helper(x)) }`
        // is rare, but `format!("{}", helper())` is not.
        let mut called = HashSet::new();
        idents(&node.tokens, &mut called);
        self.sink().extend(found);
        for name in called {
            self.call(name);
        }
    }

    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        let mut found = HashSet::new();
        literal_words(&node.value(), &mut found);
        self.sink().extend(found);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(last) = node.path.segments.last() {
            self.call(last.ident.to_string());
        }
        syn::visit::visit_expr_path(self, node);
    }
}

/// The identifiers `tokens` uses as the head of a path — `x` in `x::y` or
/// `::x::y`, and in `extern crate x` — at any depth, which is how macro output
/// names a crate. A bare identifier is far more often a local: `tokio::join!`
/// binds one called `futures`, and counting it would hide every `futures`
/// entry of every package that joins.
pub(crate) fn path_heads(tokens: &TokenStream, into: &mut HashSet<String>) {
    let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for (index, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Ident(ident) => {
                let colons = matches!(
                    (trees.get(index + 1), trees.get(index + 2)),
                    (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
                        if a.as_char() == ':' && a.spacing() == Spacing::Joint && b.as_char() == ':'
                );
                let after_extern_crate = index >= 2
                    && matches!(&trees[index - 1], TokenTree::Ident(c) if c == "crate")
                    && matches!(&trees[index - 2], TokenTree::Ident(e) if e == "extern");
                if colons || after_extern_crate {
                    into.insert(ident.to_string());
                }
            }
            TokenTree::Group(group) => path_heads(&group.stream(), into),
            _ => {}
        }
    }
}

fn idents(tokens: &TokenStream, into: &mut HashSet<String>) {
    for tree in tokens.clone() {
        match tree {
            TokenTree::Ident(ident) => {
                into.insert(ident.to_string());
            }
            TokenTree::Group(group) => idents(&group.stream(), into),
            _ => {}
        }
    }
}

fn literals(tokens: &TokenStream, into: &mut HashSet<String>) {
    for tree in tokens.clone() {
        match tree {
            TokenTree::Literal(literal) => literal_words(&literal.to_string(), into),
            TokenTree::Group(group) => literals(&group.stream(), into),
            _ => {}
        }
    }
}

/// The words of a string that could be a crate or path name —
/// `"bevy_extract"` for a manifest lookup, `"{}::Key"` for a `format!` —
/// and not of one with whitespace in it, which is prose: tokio-macros'
/// error messages say "futures" and would hide a `futures` entry.
fn literal_words(text: &str, into: &mut HashSet<String>) {
    let text = text.trim_matches('"');
    if text.chars().any(char::is_whitespace) {
        return;
    }
    into.extend(words(text).map(str::to_string));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(code: &str) -> MacroSource {
        let mut source = MacroSource::default();
        source.read(&syn::parse_file(code).unwrap());
        source
    }

    fn sorted(words: HashSet<String>) -> Vec<String> {
        let mut words: Vec<String> = words.into_iter().collect();
        words.sort();
        words
    }

    /// bevy_render_macros' shape: two derives in one crate, each crediting
    /// only what its own function and the helpers it calls write.
    #[test]
    fn each_macro_is_credited_with_what_its_own_code_writes() {
        let crate_source = source(
            "const SHARED: &str = \"shared_crate\";\n\
             #[proc_macro_derive(AsBindGroup, attributes(texture))]\n\
             pub fn bind_group(i: TokenStream) -> TokenStream { emit_image(i) }\n\
             fn emit_image(i: TokenStream) -> TokenStream { quote! { ::image_crate::Image } }\n\
             #[proc_macro_derive(SpecializerKey)]\n\
             pub fn key(i: TokenStream) -> TokenStream {\n\
                 let store = manifest(\"key_store\");\n\
                 let error = \"a sentence about prose_crate\";\n\
                 quote! { impl ::render_crate::Key for X { fn f() { let local = 1; } } }\n\
             }\n\
             #[proc_macro_attribute]\n\
             pub fn marked(a: TokenStream, i: TokenStream) -> TokenStream { i }\n",
        );
        let words = |used: &[&str]| {
            let used: Vec<String> = used.iter().map(|name| name.to_string()).collect();
            sorted(crate_source.words_for(used.iter()))
        };
        // `impl` heads a path in `impl ::render_crate::Key` and the literal
        // `1` is a word; neither can name a dependency, so neither costs one.
        assert_eq!(
            words(&["SpecializerKey"]),
            ["1", "impl", "key_store", "render_crate", "shared_crate"]
        );
        assert_eq!(words(&["texture"]), ["image_crate", "shared_crate"]);
        assert_eq!(words(&["marked"]), ["shared_crate"]);
        assert!(words(&["Unrelated"]).is_empty());
        let mut names: Vec<&String> = crate_source.entry_names().collect();
        names.sort();
        assert_eq!(
            names,
            ["AsBindGroup", "SpecializerKey", "marked", "texture"]
        );
    }

    /// One macro's output naming another's entry point: whichever order the
    /// sources come in, only what the package itself uses is credited.
    #[test]
    fn crediting_does_not_depend_on_the_order_the_macros_come_in() {
        let outer = source(
            "#[proc_macro_derive(Outer)]\n\
             pub fn outer(i: TokenStream) -> TokenStream { emit_derive(\"Inner\") }\n",
        );
        let inner = source(
            "#[proc_macro_derive(Inner)]\n\
             pub fn inner(i: TokenStream) -> TokenStream { quote! { impl ::dep_b::Trait for X {} } }\n",
        );
        for order in [[&outer, &inner], [&inner, &outer]] {
            let mut references = deps::CrateReferences::default();
            references.add_opaque_words(&["Outer".to_string()]);
            credit_all(&order, &mut references);
            assert!(
                !references.mentions("dep_b"),
                "the package never uses `Inner`"
            );
        }
    }

    /// Path heads only: a bare identifier is a local, and `extern crate x`
    /// names a crate without a `::`.
    #[test]
    fn only_path_heads_count_from_quoted_code() {
        let mut found = HashSet::new();
        let tokens: TokenStream =
            "let futures = 1; ::serde::Serialize; extern crate alloc as a; x : y; std::mem"
                .parse()
                .unwrap();
        path_heads(&tokens, &mut found);
        assert_eq!(sorted(found), ["alloc", "serde", "std"]);
    }
}
