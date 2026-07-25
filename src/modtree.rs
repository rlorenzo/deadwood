//! Module-tree resolution: which files are reachable from a crate root.
//!
//! Starting from a target's crate root (e.g. `src/lib.rs`), we follow
//! `mod name;` declarations — including ones nested inside inline modules —
//! to the files they refer to (`name.rs` or `name/mod.rs`, or the value of a
//! `#[path = "..."]` attribute). Anything under `src/` that is never reached
//! this way is a dead file.
//!
//! Known simplifications, tracked for later:
//! - `cfg`-gated `mod` declarations are always followed, so platform-specific
//!   files are never reported dead (conservative, no false positives).
//! - `#[path]` is resolved relative to the declaring file's directory, which
//!   matches rustc for the common cases but not every inline-module corner.
//! - Files included via `include!()` are not tracked yet.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

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
pub fn resolve(root: &Path, warnings: &mut Vec<String>) -> Vec<ParsedFile> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut result = Vec::new();
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
        let ast = match syn::parse_file(&source) {
            Ok(ast) => Some(ast),
            Err(err) => {
                warnings.push(format!("could not parse `{}`: {err}", path.display()));
                None
            }
        };

        if let Some(ast) = &ast {
            let file_dir = path.parent().unwrap_or(Path::new(""));
            let child_base = if is_mod_root {
                file_dir.to_path_buf()
            } else {
                file_dir.join(path.file_stem().unwrap_or_default())
            };
            collect_mod_decls(
                &ast.items,
                file_dir,
                &child_base,
                &module,
                &path,
                &mut queue,
                warnings,
            );
        }

        result.push(ParsedFile { path, ast, module });
    }

    result
}

fn collect_mod_decls(
    items: &[syn::Item],
    file_dir: &Path,
    child_base: &Path,
    module: &[String],
    declaring_file: &Path,
    queue: &mut Vec<Pending>,
    warnings: &mut Vec<String>,
) {
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        let mut child_module = module.to_vec();
        child_module.push(m.ident.to_string());
        match &m.content {
            // Inline module: its own file-backed children live one directory
            // level deeper (`mod a { mod b; }` in lib.rs -> src/a/b.rs).
            Some((_, inner)) => {
                let nested_base = child_base.join(m.ident.to_string());
                collect_mod_decls(
                    inner,
                    file_dir,
                    &nested_base,
                    &child_module,
                    declaring_file,
                    queue,
                    warnings,
                );
            }
            // External module: find the file it refers to.
            None => {
                if let Some(explicit) = path_attr(&m.attrs) {
                    let target = file_dir.join(explicit);
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
                    } else {
                        warnings.push(format!(
                            "`mod {}` in `{}` points at missing file `{}`",
                            m.ident,
                            declaring_file.display(),
                            target.display()
                        ));
                    }
                    continue;
                }
                let name = m.ident.to_string();
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
                } else {
                    warnings.push(format!(
                        "`mod {name}` in `{}` has no file at `{}` or `{}`",
                        declaring_file.display(),
                        as_file.display(),
                        as_dir.display()
                    ));
                }
            }
        }
    }
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

    #[test]
    fn path_attr_reads_string_literal() {
        let file: syn::File = syn::parse_str("#[path = \"other.rs\"]\nmod x;").unwrap();
        let syn::Item::Mod(m) = &file.items[0] else {
            panic!("expected mod item");
        };
        assert_eq!(path_attr(&m.attrs).as_deref(), Some("other.rs"));
    }
}
