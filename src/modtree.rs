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
//! # Carrying a gate into the file it names
//!
//! Following a `mod` declaration is not the only thing its gate decides. A
//! `#[cfg(test)] mod tests;` whose body lives in `tests.rs` is test code, and
//! the gate saying so is written in the *parent* file — so a detector reading
//! `tests.rs` on its own sees nothing to tell it apart from shipping code.
//! [`ParsedFile::test_only`] carries that answer down, and
//! [`crate::deps`] uses it to attribute the file's mentions of a crate to the
//! test code they are. Written inline, the same module needed nothing from
//! here: `src/deps.rs` walks the item tree itself and sees the gate.
//!
//! The corner that decides whether that is safe is a file *two* declarations
//! reach with different gates — through `#[path]` aliasing, or the same file
//! pulled into two targets. The rule is that a file reached by any non-test
//! declaration is not test-only, and [`attribute_test_only`] applies it after
//! every declaration is known rather than while walking, because the walk
//! visits each file once and queue order would otherwise pick the answer. See
//! that function for why this direction is the load-bearing one.
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

use std::collections::{HashMap, HashSet};
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
    /// Whether every `mod` declaration chain reaching this file confines it to
    /// a test build, which is what a `#[cfg(test)] mod tests;` with its body in
    /// `tests.rs` does. The gate is written in the parent, so this is the only
    /// place the file's own contents could learn it from; see the module docs
    /// and [`attribute_test_only`].
    pub test_only: bool,
}

/// A file waiting to be loaded, with the context needed to place its items.
///
/// Deliberately without a test-only flag: the queue is LIFO and each file is
/// visited once, so a flag carried here would be whichever declaration popped
/// first rather than the answer over all of them.
struct Pending {
    path: PathBuf,
    /// Whether the file owns its parent directory for child modules
    /// (`lib.rs`/`mod.rs`) or nests them in a stem-named directory.
    is_mod_root: bool,
    module: Vec<String>,
}

/// One file-backed `mod` declaration, kept until the walk is over so the gates
/// on every declaration that reaches a file can be resolved together.
struct Declaration {
    /// The file the declaration is written in.
    declared_in: PathBuf,
    /// The file it names, normalized as [`ParsedFile::path`] is.
    names: PathBuf,
    /// Whether a gate confining the code to a test build stands between the
    /// declaring file and this declaration: the `mod`'s own attributes, an
    /// inline module holding it, or an inner `#![cfg(test)]` on the file.
    test_gated: bool,
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
    let mut declarations: Vec<Declaration> = Vec::new();

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
            // An inner `#![cfg(...)]` gates the file it is written in, not one
            // item in it, so a matrix that rules it out takes the whole file
            // and every module below it. Checked before the `mod` walk, or the
            // children of a file that is not in this build would be queued.
            if !gates.compiled(&ast.attrs) {
                resolved.excluded.extend(rs_files_under(&child_base));
                resolved.excluded.push(path);
                continue;
            }
            let declaring = Declaring {
                dir: file_dir,
                file: &path,
                ignore,
                gates,
                // An inner `#![cfg(test)]` confines the whole file to a test
                // build, and with it every module declared in it.
                test_gated: gates.test_only(&ast.attrs),
            };
            let mut walk = Walk {
                queue: &mut queue,
                declarations: &mut declarations,
                excluded: &mut resolved.excluded,
                warnings,
            };
            collect_mod_decls(&ast.items, &declaring, &child_base, &module, &mut walk);
            // After the walk: the declarations above are read from the file as
            // written, and pruning would hide the excluded ones from it.
            crate::cfg::prune(gates, ast);
        }

        resolved.files.push(ParsedFile {
            path,
            ast,
            module,
            // Filled in below, once every declaration is known.
            test_only: false,
        });
    }

    attribute_test_only(&normalize(root), &declarations, &mut resolved.files);
    resolved
}

/// Mark the files no ungated chain of `mod` declarations reaches as test-only.
///
/// A file is test-only when *every* declaration chain leading to it passes
/// through a gate that confines it to a test build — equivalently, when the
/// crate root does not reach it through declarations that are all ungated. The
/// same file can be named by two declarations with different gates (`#[path]`
/// aliasing, or one file pulled into two module positions), and deciding this
/// here rather than during the walk is what keeps the answer out of the hands
/// of queue order: the walk visits a file once, so whichever declaration popped
/// first would have decided it.
///
/// The direction is load bearing. Calling a file test-only moves its mentions
/// of a crate into the dev context, which is what turns a `[dependencies]`
/// entry into a `misplaced_dependency` finding — so getting it wrong invents a
/// finding, the one outcome Deadwood must not produce. Getting it wrong the
/// other way only loses one, which is the trade every other check here makes.
fn attribute_test_only(root: &Path, declarations: &[Declaration], files: &mut [ParsedFile]) {
    let mut ungated: HashMap<&Path, Vec<&Path>> = HashMap::new();
    for declaration in declarations.iter().filter(|d| !d.test_gated) {
        ungated
            .entry(&declaration.declared_in)
            .or_default()
            .push(&declaration.names);
    }
    // Reachability from the crate root over ungated declarations only, as a
    // worklist rather than a recursive walk: `#[path]` lets two files declare
    // each other, so the module graph is not guaranteed to be acyclic.
    let mut runtime: HashSet<&Path> = HashSet::from([root]);
    let mut pending: Vec<&Path> = vec![root];
    while let Some(file) = pending.pop() {
        for named in ungated.get(file).into_iter().flatten().copied() {
            if runtime.insert(named) {
                pending.push(named);
            }
        }
    }

    for file in files {
        file.test_only = !runtime.contains(file.path.as_path());
    }
}

/// What stays fixed while one file's `mod` declarations are walked: where its
/// `#[path]` targets are resolved from, what to blame in a warning, which
/// missing files are not worth warning about, which builds are analyzed, and
/// whether the code holding these declarations is already test-only.
#[derive(Clone, Copy)]
struct Declaring<'a> {
    dir: &'a Path,
    file: &'a Path,
    ignore: Ignore<'a>,
    gates: &'a Gates<'a>,
    /// Whether a gate above these items already confines them to a test build:
    /// an inner `#![cfg(test)]` on the file, or a `#[cfg(test)]` inline module
    /// holding them. Every declaration found under one is test-gated whatever
    /// its own attributes say.
    test_gated: bool,
}

/// What a `mod` walk produces, gathered so it can be threaded through the
/// recursion as one argument.
struct Walk<'a> {
    queue: &'a mut Vec<Pending>,
    declarations: &'a mut Vec<Declaration>,
    excluded: &'a mut Vec<PathBuf>,
    warnings: &'a mut Vec<String>,
}

impl Walk<'_> {
    /// Queue a file-backed module, recording the declaration that reached it.
    fn reach(
        &mut self,
        path: PathBuf,
        is_mod_root: bool,
        module: Vec<String>,
        declaring: &Declaring<'_>,
        test_gated: bool,
    ) {
        self.declarations.push(Declaration {
            declared_in: declaring.file.to_path_buf(),
            names: normalize(&path),
            test_gated,
        });
        self.queue.push(Pending {
            path,
            is_mod_root,
            module,
        });
    }
}

fn collect_mod_decls(
    items: &[syn::Item],
    declaring: &Declaring<'_>,
    child_base: &Path,
    module: &[String],
    walk: &mut Walk<'_>,
) {
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        let name = m.ident.to_string();
        // A `mod` the configured matrix rules out is not part of this build:
        // neither it nor the files under it are read, and neither is dead.
        if !declaring.gates.compiled(&m.attrs) {
            let named = path_attr(&m.attrs).map(|explicit| declaring.dir.join(explicit));
            exclude_subtree(named.as_deref(), child_base, &name, walk.excluded);
            continue;
        }
        let mut child_module = module.to_vec();
        child_module.push(name.clone());
        // A gate that confines this declaration to a test build confines the
        // module it names, wherever that module's body lives.
        let test_gated = declaring.test_gated || declaring.gates.test_only(&m.attrs);
        match &m.content {
            // Inline module: its own file-backed children live one directory
            // level deeper (`mod a { mod b; }` in lib.rs -> src/a/b.rs).
            Some((_, inner)) => {
                let nested_base = child_base.join(&name);
                let inner_declaring = Declaring {
                    test_gated,
                    ..*declaring
                };
                collect_mod_decls(inner, &inner_declaring, &nested_base, &child_module, walk);
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
                        walk.reach(target, owns_dir, child_module, declaring, test_gated);
                    } else if !declaring.ignore.matches(&target) {
                        walk.warnings.push(format!(
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
                    walk.reach(as_file, false, child_module, declaring, test_gated);
                } else if as_dir.is_file() {
                    walk.reach(as_dir, true, child_module, declaring, test_gated);
                } else if !declaring.ignore.matches(&as_file) && !declaring.ignore.matches(&as_dir)
                {
                    walk.warnings.push(format!(
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
///
/// The cost is that an orphan file sitting in that directory — one no `mod`
/// under the excluded module actually declares — is covered too, and so goes
/// unreported. That is the right trade: the module tree here was never
/// resolved, so "nothing declares it" is a claim about code we did not read,
/// and the finding is lost rather than invented.
fn exclude_subtree(
    named: Option<&Path>,
    child_base: &Path,
    name: &str,
    excluded: &mut Vec<PathBuf>,
) {
    let (file, directory) = match named {
        Some(target) => {
            let parent = target.parent().unwrap_or(Path::new(""));
            // The same rule `collect_mod_decls` follows when it queues one:
            // only a file literally named `mod.rs` owns its parent directory
            // for children, so `#[path = "sub/mod.rs"]` nests them in `sub/`
            // and every other target nests them in a stem-named directory.
            let directory = if target.file_name().is_some_and(|n| n == "mod.rs") {
                parent.to_path_buf()
            } else {
                parent.join(target.file_stem().unwrap_or_default())
            };
            (target.to_path_buf(), directory)
        }
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

    /// A `mod` outside the analyzed build has to account for the same child
    /// directory `collect_mod_decls` would have queued from, or the files
    /// under it come back as dead — reported from a module tree we
    /// deliberately did not read. The `mod.rs` case is the one that differs:
    /// that file owns its parent directory instead of nesting in a stem-named
    /// one.
    #[test]
    fn an_excluded_module_accounts_for_the_directory_its_children_live_in() {
        let directory_of = |named: Option<&str>, name: &str| {
            let mut excluded = Vec::new();
            exclude_subtree(
                named.map(Path::new),
                Path::new("/ws/src"),
                name,
                &mut excluded,
            );
            // Nothing exists on disk, so only the file itself is listed; the
            // directory is what the walk was pointed at.
            excluded
        };

        assert_eq!(
            directory_of(None, "win"),
            vec![PathBuf::from("/ws/src/win.rs")]
        );
        assert_eq!(
            directory_of(Some("/ws/src/renamed.rs"), "win"),
            vec![PathBuf::from("/ws/src/renamed.rs")]
        );

        // The behavior that matters is which directory gets walked, so check
        // it against a tree that exists: this crate's own `src/`.
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut through_mod_rs = Vec::new();
        exclude_subtree(
            Some(&src.join("mod.rs")),
            Path::new("/ws/elsewhere"),
            "src",
            &mut through_mod_rs,
        );
        assert!(
            through_mod_rs.contains(&normalize(&src.join("modtree.rs"))),
            "`#[path = \"src/mod.rs\"]` nests its children in `src/`, not in \
             `src/mod/`: {through_mod_rs:?}"
        );
    }

    /// Every file of a fixture crate as `(file name, test_only)`, sorted, with
    /// the default matrix and no ignore patterns.
    fn test_only_flags(fixture: &str) -> Vec<(String, bool)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/{fixture}/src/lib.rs"));
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let mut flags: Vec<(String, bool)> = resolved
            .files
            .iter()
            .map(|file| {
                let name = file.path.file_name().unwrap_or_default();
                (name.to_string_lossy().into_owned(), file.test_only)
            })
            .collect();
        flags.sort();
        flags
    }

    /// The gap this closes: the gate is written in the parent file, so the file
    /// it names has nothing in it to say the code is test-only.
    #[test]
    fn a_cfg_test_declaration_makes_the_file_it_names_test_only() {
        let flags = test_only_flags("modgate");
        assert!(
            flags.contains(&("tests.rs".to_string(), true)),
            "`#[cfg(test)] mod tests;` in lib.rs confines src/tests.rs: {flags:?}"
        );
    }

    /// The corner that decides whether the flag is safe. A file two
    /// declarations reach is read once, so an answer taken while walking would
    /// be whichever declaration the LIFO queue popped first; the answer over
    /// all of them is that an ungated declaration compiles the file into a
    /// runtime build, and calling it test-only would invent a placement
    /// finding.
    #[test]
    fn a_file_reached_by_an_ungated_declaration_is_not_test_only() {
        let flags = test_only_flags("modgate");
        assert!(
            flags.contains(&("both_ways.rs".to_string(), false)),
            "one gated and one ungated declaration reach src/both_ways.rs: {flags:?}"
        );
    }

    /// A gate is inherited by everything below it: through an inline module,
    /// whose file-backed children are walked in the same pass, and through a
    /// file that gates itself with an inner `#![cfg(test)]`. The file carrying
    /// that inner attribute is not itself marked — a detector reading it can
    /// see the attribute, which is the one thing a declaration in another file
    /// never gives it.
    #[test]
    fn a_gate_reaches_every_file_declared_under_it() {
        assert_eq!(
            test_only_flags("modgate"),
            vec![
                ("both_ways.rs".to_string(), false),
                ("helper.rs".to_string(), true),
                ("inherited.rs".to_string(), true),
                ("inner_gated.rs".to_string(), false),
                ("lib.rs".to_string(), false),
                ("tests.rs".to_string(), true),
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
