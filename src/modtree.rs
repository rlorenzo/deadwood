//! Module-tree resolution: which files are reachable from a crate root.
//!
//! Starting from a target's crate root (e.g. `src/lib.rs`), we follow
//! `mod name;` declarations — including ones nested inside inline modules —
//! to the files they refer to (`name.rs` or `name/mod.rs`, or the value of a
//! `#[path = "..."]` attribute). Anything under `src/` that is never reached
//! this way is a dead file.
//!
//! `cfg`-gated `mod` declarations are followed whenever the gate can hold in
//! some build the configured matrix admits ([`crate::cfg`]), which with the
//! default matrix is every gate that can hold anywhere — so platform-specific
//! files are still never reported dead. A `mod` the matrix *does* rule out is
//! not followed and not reported dead either: the file it names, and the
//! directory its children would live in, are returned in [`Resolved::excluded`]
//! so the dead-file check can tell "not in this build" from "reachable by
//! nothing". Each file's AST is then pruned of the items the matrix leaves
//! out, so every detector downstream sees only the build being analyzed.
//!
//! Known simplifications, tracked for later:
//! - `#[path]` is resolved relative to the declaring file's directory, which
//!   matches rustc for the common cases but not every inline-module corner.
//! - Files included via `include!()` are not tracked yet.
//!
//! Configured `ignore` patterns touch exactly one thing here: a `mod`
//! declaration pointing at a *missing* file that an ignore pattern covers is
//! skipped silently instead of warned about. Without that, ignoring a
//! generated module would leave the declaration behind as an unresolved-module
//! warning, which skips the whole package's checks — an ignored file turning
//! into a reason to stop analyzing everything around it. Files that do exist
//! are read as usual whether ignored or not: their contents are still evidence
//! about the code that is *not* ignored (see [`crate::config`]).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cfg::Gates;
use crate::config::Ignore;

/// What module resolution found from one crate root.
pub struct Resolved {
    /// Every file reached, in the analyzed build.
    pub files: Vec<ParsedFile>,
    /// Files a `cfg` the configured matrix rules out keeps out of the
    /// analysis. They are neither read nor analyzed — and, crucially, not
    /// dead either: nothing reaches them because this build does not contain
    /// them, which is not the same as nothing reaching them at all.
    pub excluded: Vec<PathBuf>,
}

/// A source file reached during module resolution.
pub struct ParsedFile {
    pub path: PathBuf,
    /// `None` when the file failed to parse; it still counts as reachable.
    pub ast: Option<syn::File>,
    /// Module path of the file's items relative to the crate root, taken from
    /// the `mod` declarations that led here (empty for the crate root itself).
    /// Names come from the declarations, not the file names, so `#[path]`
    /// aliases land under the name paths actually use.
    pub module: Vec<String>,
}

/// A file waiting to be loaded, with the context needed to place its items.
struct Pending {
    path: PathBuf,
    /// Whether the file owns its parent directory for child modules
    /// (`lib.rs`/`mod.rs`) or nests them in a stem-named directory.
    is_mod_root: bool,
    module: Vec<String>,
}

/// Follow `mod` declarations from `root` and return every file reached.
///
/// Unresolvable modules and unparsable files are reported through `warnings`
/// rather than failing the whole analysis.
pub fn resolve(
    root: &Path,
    ignore: Ignore<'_>,
    gates: &Gates<'_>,
    warnings: &mut Vec<String>,
) -> Resolved {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut resolved = Resolved {
        files: Vec::new(),
        excluded: Vec::new(),
    };
    let mut queue: Vec<Pending> = vec![Pending {
        path: root.to_path_buf(),
        is_mod_root: true,
        module: Vec::new(),
    }];

    while let Some(Pending {
        path,
        is_mod_root,
        module,
    }) = queue.pop()
    {
        let path = normalize(&path);
        if !visited.insert(path.clone()) {
            continue;
        }
        let source = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) => {
                warnings.push(format!("could not read `{}`: {err}", path.display()));
                continue;
            }
        };
        let mut ast = match syn::parse_file(&source) {
            Ok(ast) => Some(ast),
            Err(err) => {
                warnings.push(format!("could not parse `{}`: {err}", path.display()));
                None
            }
        };

        if let Some(ast) = &mut ast {
            let file_dir = path.parent().unwrap_or(Path::new(""));
            let child_base = if is_mod_root {
                file_dir.to_path_buf()
            } else {
                file_dir.join(path.file_stem().unwrap_or_default())
            };
            let declaring = Declaring {
                dir: file_dir,
                file: &path,
                ignore,
                gates,
            };
            collect_mod_decls(
                &ast.items,
                &declaring,
                &child_base,
                &module,
                &mut queue,
                &mut resolved.excluded,
                warnings,
            );
            // After the walk: the declarations above are read from the file as
            // written, and pruning would hide the excluded ones from it.
            crate::cfg::prune(gates, ast);
        }

        resolved.files.push(ParsedFile { path, ast, module });
    }

    resolved
}

/// What stays fixed while one file's `mod` declarations are walked: where its
/// `#[path]` targets are resolved from, what to blame in a warning, which
/// missing files are not worth warning about, and which builds are analyzed.
struct Declaring<'a> {
    dir: &'a Path,
    file: &'a Path,
    ignore: Ignore<'a>,
    gates: &'a Gates<'a>,
}

fn collect_mod_decls(
    items: &[syn::Item],
    declaring: &Declaring<'_>,
    child_base: &Path,
    module: &[String],
    queue: &mut Vec<Pending>,
    excluded: &mut Vec<PathBuf>,
    warnings: &mut Vec<String>,
) {
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        let name = m.ident.to_string();
        // A `mod` the configured matrix rules out is not part of this build:
        // neither it nor the files under it are read, and neither is dead.
        if !declaring.gates.compiled(&m.attrs) {
            let named = path_attr(&m.attrs).map(|explicit| declaring.dir.join(explicit));
            exclude_subtree(named.as_deref(), child_base, &name, excluded);
            continue;
        }
        let mut child_module = module.to_vec();
        child_module.push(name.clone());
        match &m.content {
            // Inline module: its own file-backed children live one directory
            // level deeper (`mod a { mod b; }` in lib.rs -> src/a/b.rs).
            Some((_, inner)) => {
                let nested_base = child_base.join(&name);
                collect_mod_decls(
                    inner,
                    declaring,
                    &nested_base,
                    &child_module,
                    queue,
                    excluded,
                    warnings,
                );
            }
            // External module: find the file it refers to.
            None => {
                if let Some(explicit) = path_attr(&m.attrs) {
                    let target = declaring.dir.join(explicit);
                    if target.is_file() {
                        // Only a file literally named `mod.rs` owns its parent
                        // directory; any other `#[path]` target keeps the
                        // stem-based rule for its own children.
                        let owns_dir = target.file_name().is_some_and(|n| n == "mod.rs");
                        queue.push(Pending {
                            path: target,
                            is_mod_root: owns_dir,
                            module: child_module,
                        });
                    } else if !declaring.ignore.matches(&target) {
                        warnings.push(format!(
                            "`mod {}` in `{}` points at missing file `{}`",
                            m.ident,
                            declaring.file.display(),
                            target.display()
                        ));
                    }
                    continue;
                }
                let as_file = child_base.join(format!("{name}.rs"));
                let as_dir = child_base.join(&name).join("mod.rs");
                if as_file.is_file() {
                    queue.push(Pending {
                        path: as_file,
                        is_mod_root: false,
                        module: child_module,
                    });
                } else if as_dir.is_file() {
                    queue.push(Pending {
                        path: as_dir,
                        is_mod_root: true,
                        module: child_module,
                    });
                } else if !declaring.ignore.matches(&as_file) && !declaring.ignore.matches(&as_dir)
                {
                    warnings.push(format!(
                        "`mod {name}` in `{}` has no file at `{}` or `{}`",
                        declaring.file.display(),
                        as_file.display(),
                        as_dir.display()
                    ));
                }
            }
        }
    }
}

/// Every file a `mod` outside the analyzed build takes with it.
///
/// The file it names — `#[path]` target, `name.rs`, or `name/mod.rs` — plus
/// everything under the directory its own children would live in. Listing them
/// by layout rather than by reading them is deliberate: a module that is not
/// part of this build should not be parsed, and a file that is not there is
/// still a path the dead-file check must not report.
fn exclude_subtree(
    named: Option<&Path>,
    child_base: &Path,
    name: &str,
    excluded: &mut Vec<PathBuf>,
) {
    let (file, directory) = match named {
        Some(target) => (
            target.to_path_buf(),
            target
                .parent()
                .unwrap_or(Path::new(""))
                .join(target.file_stem().unwrap_or_default()),
        ),
        None => (child_base.join(format!("{name}.rs")), child_base.join(name)),
    };
    excluded.push(normalize(&file));
    excluded.extend(rs_files_under(&directory));
}

/// Extract the value of a `#[path = "..."]` attribute, if present.
fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("path") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(lit) = &nv.value
            && let syn::Lit::Str(s) = &lit.lit
        {
            return Some(s.value());
        }
    }
    None
}

/// All `.rs` files under `dir`, recursively, skipping hidden directories.
pub fn rs_files_under(dir: &Path) -> Vec<PathBuf> {
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
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") {
                files.push(normalize(&path));
            }
        }
    }
    files.sort();
    files
}

/// Remove `.` and resolve `..` components so paths built by joining compare
/// equal to paths found by walking the directory tree.
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_removes_dot_and_dotdot() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d.rs")),
            PathBuf::from("/a/c/d.rs")
        );
    }

    /// Module paths come from the `mod` declarations that led to a file, not
    /// from its name: `#[path = "renamed_file.rs"] mod alias;` puts the file's
    /// items under `alias`, which is what paths in the crate spell.
    #[test]
    fn module_paths_follow_declarations_not_file_names() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pathmod/src/lib.rs");
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let mut modules: Vec<Vec<String>> =
            resolved.files.iter().map(|f| f.module.clone()).collect();
        modules.sort();
        assert_eq!(
            modules,
            vec![
                vec![],
                vec!["alias".to_string()],
                vec!["alias".to_string(), "child".to_string()],
            ]
        );
    }

    #[test]
    fn path_attr_reads_string_literal() {
        let file: syn::File = syn::parse_str("#[path = \"other.rs\"]\nmod x;").unwrap();
        let syn::Item::Mod(m) = &file.items[0] else {
            panic!("expected mod item");
        };
        assert_eq!(path_attr(&m.attrs).as_deref(), Some("other.rs"));
    }
}
