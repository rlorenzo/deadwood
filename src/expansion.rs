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

    /// Whether the crate defines any macro at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
    for word in text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
    {
        into.insert(word.to_string());
    }
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
