//! Unused `pub` item detection.
//!
//! Rustc's `dead_code` lint already covers private and crate-visible items,
//! but it assumes fully-`pub` items are used, because it cannot see the
//! consumers. Within a workspace we can do better: a `pub` item whose name is
//! never mentioned anywhere else in the workspace is either dead or external
//! API. Deadwood reports it and lets the author decide.
//!
//! The check is a *name-based* heuristic, deliberately biased toward false
//! negatives (staying quiet) over false positives:
//!
//! - Every identifier token in the workspace counts as a use, including
//!   tokens inside macro invocations. An item is flagged only when its name
//!   appears exactly once (its own definition).
//! - Name collisions therefore hide dead code rather than causing noise.
//! - A struct with an `impl` block is never flagged (the `impl` mentions the
//!   name), and re-exported-but-unused items are missed. Both are accepted
//!   for v0.1 and tracked as future work (proper path resolution).
//!
//! Items carrying `#[no_mangle]`, `#[used]`, `#[export_name]`, or an
//! `allow`/`expect` for `dead_code`/`unused` are skipped, as is `fn main`.

use std::collections::HashMap;
use std::path::PathBuf;

use proc_macro2::{TokenStream, TokenTree};

use crate::modtree::ParsedFile;

/// A `pub` item that nothing else in the workspace refers to.
pub struct UnusedPubItem {
    pub name: String,
    /// Item kind for display: "fn", "struct", ...
    pub kind: &'static str,
    pub file: PathBuf,
    pub line: usize,
}

/// Report `pub` items whose name never occurs outside their own definition.
pub fn find_unused_pub_items(
    files: &[ParsedFile],
    warnings: &mut Vec<String>,
) -> Vec<UnusedPubItem> {
    let mut items = Vec::new();
    for file in files {
        if let Some(ast) = &file.ast {
            collect_pub_items(&ast.items, file, &mut items);
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for file in files {
        match tokenize(&file.source) {
            Ok(tokens) => count_idents(tokens, &mut counts),
            Err(err) => warnings.push(format!(
                "could not tokenize `{}` for usage counting: {err}",
                file.path.display()
            )),
        }
    }

    items.retain(|item| counts.get(&item.name).copied().unwrap_or(0) <= 1);
    items
}

fn collect_pub_items(items: &[syn::Item], file: &ParsedFile, out: &mut Vec<UnusedPubItem>) {
    for item in items {
        let (ident, kind, attrs, vis) = match item {
            syn::Item::Fn(i) => (&i.sig.ident, "fn", &i.attrs, &i.vis),
            syn::Item::Struct(i) => (&i.ident, "struct", &i.attrs, &i.vis),
            syn::Item::Enum(i) => (&i.ident, "enum", &i.attrs, &i.vis),
            syn::Item::Trait(i) => (&i.ident, "trait", &i.attrs, &i.vis),
            syn::Item::Type(i) => (&i.ident, "type alias", &i.attrs, &i.vis),
            syn::Item::Const(i) => (&i.ident, "const", &i.attrs, &i.vis),
            syn::Item::Static(i) => (&i.ident, "static", &i.attrs, &i.vis),
            syn::Item::Union(i) => (&i.ident, "union", &i.attrs, &i.vis),
            syn::Item::Mod(m) => {
                // Descend into inline modules regardless of their visibility;
                // each item is judged on its own `pub`-ness.
                if let Some((_, inner)) = &m.content {
                    collect_pub_items(inner, file, out);
                }
                continue;
            }
            _ => continue,
        };

        if !matches!(vis, syn::Visibility::Public(_)) {
            continue;
        }
        let name = ident.to_string();
        if name == "main" || has_skip_attr(attrs) {
            continue;
        }
        out.push(UnusedPubItem {
            line: ident.span().start().line,
            name,
            kind,
            file: file.path.clone(),
        });
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

/// Parse source text into a token stream, tolerating a leading shebang.
fn tokenize(source: &str) -> Result<TokenStream, proc_macro2::LexError> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let source = if source.starts_with("#!") && !source.starts_with("#![") {
        source.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        source
    };
    source.parse()
}

fn count_idents(tokens: TokenStream, counts: &mut HashMap<String, usize>) {
    for tree in tokens {
        match tree {
            TokenTree::Ident(ident) => {
                *counts.entry(ident.to_string()).or_insert(0) += 1;
            }
            TokenTree::Group(group) => count_idents(group.stream(), counts),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(path: &str, source: &str) -> ParsedFile {
        ParsedFile {
            path: PathBuf::from(path),
            source: source.to_string(),
            ast: syn::parse_file(source).ok(),
        }
    }

    #[test]
    fn flags_unreferenced_pub_fn_only() {
        let files = vec![parsed(
            "/ws/src/lib.rs",
            "pub fn dead() {}\npub fn alive() {}\nfn caller() { alive(); }\n",
        )];
        let mut warnings = Vec::new();
        let unused = find_unused_pub_items(&files, &mut warnings);
        assert_eq!(
            unused.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(),
            vec!["dead"]
        );
        assert_eq!(unused[0].line, 1);
        assert!(warnings.is_empty());
    }

    #[test]
    fn usage_inside_macro_body_counts() {
        let files = vec![parsed(
            "/ws/src/lib.rs",
            "pub fn helper() {}\nfn go() { println!(\"{}\", helper as usize); }\n",
        )];
        let mut warnings = Vec::new();
        assert!(find_unused_pub_items(&files, &mut warnings).is_empty());
    }

    #[test]
    fn skips_allow_dead_code_and_main() {
        let files = vec![parsed(
            "/ws/src/main.rs",
            "#[allow(dead_code)]\npub fn kept() {}\npub fn main() {}\n",
        )];
        let mut warnings = Vec::new();
        assert!(find_unused_pub_items(&files, &mut warnings).is_empty());
    }

    #[test]
    fn non_pub_items_are_ignored() {
        let files = vec![parsed(
            "/ws/src/lib.rs",
            "pub(crate) fn crate_only() {}\nfn private() {}\n",
        )];
        let mut warnings = Vec::new();
        assert!(find_unused_pub_items(&files, &mut warnings).is_empty());
    }
}
