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
//! A gate also says something about the file it leads to that the file itself
//! cannot: `#[cfg(test)] mod tests;` makes `tests.rs` test code, and nothing
//! inside `tests.rs` records that. Each reached file therefore carries
//! [`ParsedFile::test_only`] down to the detectors that need it.
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
    /// Whether every `mod` declaration that leads here confines the file to a
    /// test build ([`Gates::test_only`]).
    ///
    /// The gate is written in the *parent* file, so nothing inside this one
    /// says it: `#[cfg(test)] mod tests;` in `lib.rs` makes `tests.rs` test
    /// code, and `tests.rs` looks like any other file. [`crate::deps`] needs
    /// this to attribute what it names to `[dev-dependencies]`, the same way
    /// it already attributes an inline `#[cfg(test)] mod tests { ... }`.
    ///
    /// False for the crate root, and false for any file *some* declaration no
    /// gate confines to a test build also reaches — see [`resolve`] for why
    /// that direction is the safe one.
    pub test_only: bool,
    /// Module paths, on the same basis as [`ParsedFile::module`], of the
    /// *inline* `mod` blocks in this file that a gate confines to a test
    /// build.
    ///
    /// The out-of-line spelling of a `#[cfg(test)] mod` gets its answer from
    /// `test_only` above; this is the same answer for the inline spelling, and
    /// it is recorded here rather than recomputed downstream so that
    /// [`Gates::test_only`] stays the only copy of the predicate — the walk
    /// below already evaluates it for every `mod` it passes, inline ones
    /// included, and used to throw the inline results away.
    ///
    /// Confinement accumulates, so a module nested inside a test-only one is
    /// listed too. Two declarations in one file can share a module path when
    /// disjoint `cfg`s make them alternatives of each other, and [`resolve`]
    /// applies the same rule it applies to a file two declarations reach: any
    /// one of them no gate confines to a test build clears the path, whether
    /// it carries a gate of its own or none at all.
    pub test_only_mods: Vec<Vec<String>>,
}

/// A file waiting to be loaded, with the context needed to place its items.
struct Pending {
    path: PathBuf,
    /// Whether the file owns its parent directory for child modules
    /// (`lib.rs`/`mod.rs`) or nests them in a stem-named directory.
    is_mod_root: bool,
    module: Vec<String>,
    /// Whether the declaration that queued this file — and every declaration
    /// above it — confines it to a test build.
    test_only: bool,
}

/// A file already loaded, and what a second declaration of it needs to know.
#[derive(Clone, Copy)]
struct Seen {
    /// Where it sits in [`Resolved::files`].
    index: usize,
    /// The rule its own children were resolved by, needed to redo that walk.
    is_mod_root: bool,
}

/// Follow `mod` declarations from `root` and return every file reached.
///
/// Unresolvable modules and unparsable files are reported through `warnings`
/// rather than failing the whole analysis.
///
/// A file two declarations in this walk reach is loaded once, by whichever
/// popped first. That is a real choice for [`ParsedFile::test_only`], because
/// the two declarations can disagree: `#[path]` naming one file from two
/// modules is the way it happens. The rule is that any declaration no gate
/// confines to a test build clears the flag, however
/// late it arrives: a file wrongly marked test-only would move the crates it
/// names into the dev context and could have a `[dependencies]` entry reported
/// as belonging in `[dev-dependencies]` when the library genuinely uses it.
/// That is a false positive, where the other direction is a missed finding.
/// Clearing it late means redoing that file's `mod` walk so its children are
/// cleared too, which each file can need at most once.
///
/// This is one walk from one crate root, so it says nothing across targets. A
/// file that two targets both compile is resolved once per target and can come
/// out test-only in one and not the other, which is the right answer: what a
/// file is depends on what reached it, and each target reached it its own way.
pub fn resolve(
    root: &Path,
    ignore: Ignore<'_>,
    gates: &Gates<'_>,
    warnings: &mut Vec<String>,
) -> Resolved {
    // Every path popped, whatever became of it, so that a file reached twice
    // is read once — including one that could not be read at all, or that its
    // own inner `#![cfg(...)]` keeps out of this build. Those two never reach
    // `seen`, and without this they would be re-read and re-reported once per
    // declaration naming them.
    let mut visited: HashSet<PathBuf> = HashSet::new();
    // The subset that became a file, and where it landed.
    let mut seen: HashMap<PathBuf, Seen> = HashMap::new();
    let mut resolved = Resolved {
        files: Vec::new(),
        excluded: Vec::new(),
    };
    let mut queue: Vec<Pending> = vec![Pending {
        path: root.to_path_buf(),
        is_mod_root: true,
        module: Vec::new(),
        test_only: false,
    }];

    while let Some(Pending {
        path,
        is_mod_root,
        module,
        test_only,
    }) = queue.pop()
    {
        let path = normalize(&path);
        if !visited.insert(path.clone()) {
            // Reached again. Nothing about the file changes — it is the same
            // file — except that a declaration no gate confines to a test
            // build overrides a test-only one recorded earlier, and its
            // children with it. A path that
            // never became a file has nothing to override.
            let Some(&Seen { index, is_mod_root }) = seen.get(&path) else {
                continue;
            };
            if test_only || !resolved.files[index].test_only {
                continue;
            }
            resolved.files[index].test_only = false;
            let mut inline_mods = Vec::new();
            let file = &resolved.files[index];
            if let Some(ast) = &file.ast {
                let declaring = Declaring {
                    dir: parent_of(&path),
                    file: &path,
                    ignore,
                    gates,
                };
                // The subtree below was already excluded and warned about on
                // the first walk; repeating either would double it up.
                collect_mod_decls(
                    &ast.items,
                    &declaring,
                    Under {
                        base: &child_base(&path, is_mod_root),
                        module: &file.module,
                        test_only: false,
                    },
                    &mut Walk {
                        queue: &mut queue,
                        excluded: &mut Vec::new(),
                        warnings: &mut Vec::new(),
                        inline_mods: &mut inline_mods,
                    },
                );
            }
            // The inline list, unlike the two above, is *replaced*: the first
            // walk recorded every inline module under a file that was itself
            // confined, and lifting the file leaves only the ones their own
            // gate confines.
            resolved.files[index].test_only_mods = confined_inline_mods(inline_mods);
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

        let mut inline_mods = Vec::new();
        if let Some(ast) = &mut ast {
            let file_dir = path.parent().unwrap_or(Path::new(""));
            let child_base = child_base(&path, is_mod_root);
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
            };
            collect_mod_decls(
                &ast.items,
                &declaring,
                Under {
                    base: &child_base,
                    module: &module,
                    test_only,
                },
                &mut Walk {
                    queue: &mut queue,
                    excluded: &mut resolved.excluded,
                    warnings,
                    inline_mods: &mut inline_mods,
                },
            );
            // After the walk: the declarations above are read from the file as
            // written, and pruning would hide the excluded ones from it.
            crate::cfg::prune(gates, ast);
        }

        seen.insert(
            path.clone(),
            Seen {
                index: resolved.files.len(),
                is_mod_root,
            },
        );
        resolved.files.push(ParsedFile {
            path,
            ast,
            module,
            test_only,
            test_only_mods: confined_inline_mods(inline_mods),
        });
    }

    resolved
}

/// Where a file's file-backed child modules live: beside it when it owns its
/// directory (`lib.rs`, `mod.rs`), in a stem-named directory otherwise.
fn child_base(path: &Path, is_mod_root: bool) -> PathBuf {
    let dir = parent_of(path);
    if is_mod_root {
        dir.to_path_buf()
    } else {
        dir.join(path.file_stem().unwrap_or_default())
    }
}

fn parent_of(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
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

/// What the declarations of one file inherit from the ones above them: where
/// their files are looked for, what module path their items land under, and
/// whether they are already confined to a test build.
#[derive(Clone, Copy)]
struct Under<'a> {
    base: &'a Path,
    module: &'a [String],
    test_only: bool,
}

/// What a walk produces: files still to load, files this build leaves out, and
/// declarations that resolved to nothing.
struct Walk<'a> {
    queue: &'a mut Vec<Pending>,
    excluded: &'a mut Vec<PathBuf>,
    warnings: &'a mut Vec<String>,
    /// Every inline `mod` the walk passed, by module path, and whether it was
    /// confined to a test build. Reduced to [`ParsedFile::test_only_mods`] by
    /// [`confined_inline_mods`].
    inline_mods: &'a mut Vec<(Vec<String>, bool)>,
}

/// The inline modules a gate confines to a test build, with the alternatives
/// rule applied.
///
/// One file can declare `mod imp` twice under disjoint `cfg`s, and the symbol
/// table merges the two into one module, so a path both spellings reach cannot
/// be answered two ways. Any declaration *no gate confines to a test build*
/// clears it — the same direction [`resolve`] takes for a file two
/// declarations disagree about, and for the same reason: an entry point
/// wrongly read as test code is a false positive, where the other direction is
/// a missed finding.
///
/// "Not confined" is wider than "ungated", and deliberately: a declaration can
/// carry a gate of its own and still be compiled by a build with no tests in
/// it — `#[cfg(all(not(test), unix))]`, or the `any(test, ...)` shape
/// [`Gates::test_only`] already answers `false` for. Those clear the path too,
/// because what they contribute to the merged module is production code.
fn confined_inline_mods(inline_mods: Vec<(Vec<String>, bool)>) -> Vec<Vec<String>> {
    let unconfined: HashSet<&[String]> = inline_mods
        .iter()
        .filter(|(_, test_only)| !test_only)
        .map(|(path, _)| path.as_slice())
        .collect();
    inline_mods
        .iter()
        .filter(|(path, test_only)| *test_only && !unconfined.contains(path.as_slice()))
        .map(|(path, _)| path.clone())
        .collect()
}

fn collect_mod_decls(
    items: &[syn::Item],
    declaring: &Declaring<'_>,
    under: Under<'_>,
    walk: &mut Walk<'_>,
) {
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        let name = m.ident.to_string();
        // Test-confinement accumulates downward and never lifts: a module
        // inside `#[cfg(test)] mod tests` is test code whatever its own gate
        // says, so the declaration only ever adds to what it inherited.
        let test_only = under.test_only || declaring.gates.test_only(&m.attrs);
        // A `mod` the configured matrix rules out is not part of this build:
        // neither it nor the files under it are read, and neither is dead.
        if !declaring.gates.compiled(&m.attrs) {
            let named = path_attr(&m.attrs).map(|explicit| declaring.dir.join(explicit));
            exclude_subtree(named.as_deref(), under.base, &name, walk.excluded);
            continue;
        }
        let mut child_module = under.module.to_vec();
        child_module.push(name.clone());
        match &m.content {
            // Inline module: its own file-backed children live one directory
            // level deeper (`mod a { mod b; }` in lib.rs -> src/a/b.rs).
            Some((_, inner)) => {
                let nested_base = under.base.join(&name);
                walk.inline_mods.push((child_module.clone(), test_only));
                collect_mod_decls(
                    inner,
                    declaring,
                    Under {
                        base: &nested_base,
                        module: &child_module,
                        test_only,
                    },
                    walk,
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
                        walk.queue.push(Pending {
                            path: target,
                            is_mod_root: owns_dir,
                            module: child_module,
                            test_only,
                        });
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
                let as_file = under.base.join(format!("{name}.rs"));
                let as_dir = under.base.join(&name).join("mod.rs");
                if as_file.is_file() {
                    walk.queue.push(Pending {
                        path: as_file,
                        is_mod_root: false,
                        module: child_module,
                        test_only,
                    });
                } else if as_dir.is_file() {
                    walk.queue.push(Pending {
                        path: as_dir,
                        is_mod_root: true,
                        module: child_module,
                        test_only,
                    });
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

    /// Which files a build reaches only through test-confined declarations,
    /// against a fixture that puts every case in one crate: a `#[cfg(test)]
    /// mod` with its body in its own file, and a file three declarations reach
    /// — a confining one either side of one that does not confine, so no pop
    /// order can be the thing that answers it.
    #[test]
    fn test_confinement_follows_declarations_and_an_unconfined_one_clears_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/depkinds/src/lib.rs");
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let mut reached: Vec<(String, bool)> = resolved
            .files
            .iter()
            .map(|file| {
                let name = file
                    .path
                    .strip_prefix(root.parent().unwrap())
                    .unwrap_or(&file.path)
                    .to_string_lossy()
                    .replace('\\', "/");
                (name, file.test_only)
            })
            .collect();
        reached.sort();
        assert_eq!(
            reached,
            vec![
                ("lib.rs".to_string(), false),
                // Its only declaration is `#[cfg(test)] mod outline_tests;`.
                ("outline_tests.rs".to_string(), true),
                // Reached by a declaration that does not confine it, among
                // ones that do.
                ("shared_view.rs".to_string(), false),
                // And so is its child, which carries no gate of its own.
                ("shared_view/deeper.rs".to_string(), false),
            ]
        );

        // The inline list is *replaced* when a file is lifted, not added to.
        // `shared_view.rs` is walked once under a gated declaration — which
        // records every inline module in it as confined, because the file was
        // — and again when the unconfined one clears it.
        let shared = resolved
            .files
            .iter()
            .find(|file| file.path.ends_with("shared_view.rs"))
            .expect("the fixture declares the file three times");
        assert!(
            shared.test_only_mods.is_empty(),
            "`inline_view` sits in a file no gate confines: {:?}",
            shared.test_only_mods,
        );
    }

    /// The inline half of the same question, and the four gate shapes that
    /// make [`Gates::test_only`] worth reusing rather than matching
    /// `#[cfg(test)]` by shape: `test` alone confines a module, `any(test,
    /// ...)` does not because the gate holds without the tests, `not(test)` is
    /// the opposite gate, and an ungated module inside a confined one is
    /// confined because confinement accumulates downward.
    #[test]
    fn an_inline_mods_gate_is_recorded_for_the_module_paths_it_confines() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/testonly/app/src/main.rs");
        let config = crate::config::Config::default();
        // `extra` is declared by the fixture's own manifest, and it has to be
        // declared here too: a feature no build can turn on makes `all(test,
        // feature = "extra")` a gate that holds nowhere at all, which is a
        // different question with a different answer.
        let mut package = crate::cfg::tests_support::bare_package();
        package.features.insert("extra".to_string(), Vec::new());
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let inline = resolved
            .files
            .iter()
            .find(|file| file.module == ["inline"])
            .expect("the fixture declares `mod inline;`");
        let mut confined = inline.test_only_mods.clone();
        confined.sort();
        assert_eq!(
            confined,
            vec![
                // `#[cfg(test)] mod gated { ... }`.
                vec!["inline".to_string(), "gated".to_string()],
                // Ungated, and inside `gated`.
                vec![
                    "inline".to_string(),
                    "gated".to_string(),
                    "deeper".to_string()
                ],
                // `#[cfg(all(test, feature = "extra"))] mod narrow { ... }`:
                // a gate that narrows a test build is still confined to one.
                vec!["inline".to_string(), "narrow".to_string()],
                // `#[cfg(test)] mod tests { ... }`.
                vec!["inline".to_string(), "tests".to_string()],
            ],
            "`either_way` is `any(test, unix)` and `never_in_tests` is \
             `not(test)`; neither is confined to a test build. `alt` is \
             declared twice — `#[cfg(test)]` and `#[cfg(all(not(test), \
             unix))]` — and the second is gated rather than ungated, so a \
             rule that cleared the path only for an *ungated* alternative \
             would list it here",
        );

        // And the out-of-line spelling of `gated`, which is the same answer
        // carried by the other field.
        let outline = resolved
            .files
            .iter()
            .find(|file| file.module == ["inline", "outline"])
            .expect("the fixture declares `#[cfg(test)] mod outline;`");
        assert!(outline.test_only, "the two spellings have to agree");
    }

    /// One file can declare `mod imp` twice under disjoint `cfg`s, and the
    /// symbol table merges the two into one module — so a module path a
    /// confined and an unconfined declaration both reach cannot be answered
    /// two ways. The unconfined one clears it, which is the direction that
    /// loses a finding rather than inventing one.
    ///
    /// Unconfined is not the same as ungated, and the fixture case in
    /// [`an_inline_mods_gate_is_recorded_for_the_module_paths_it_confines`]
    /// is the half that tells them apart: this function is downstream of
    /// [`Gates::test_only`] and sees only its answer.
    #[test]
    fn a_declaration_no_gate_confines_clears_a_path_a_confined_one_also_reaches() {
        let confined = confined_inline_mods(vec![
            (vec!["alt".to_string()], true),
            (vec!["alt".to_string()], false),
            (vec!["gated".to_string()], true),
        ]);
        assert_eq!(confined, vec![vec!["gated".to_string()]]);
    }

    /// A path is popped once whatever becomes of it, which the two paths that
    /// never reach `seen` — a file that cannot be read, and one its own inner
    /// `#![cfg(...)]` keeps out of this build — depend on. Without that, every
    /// declaration naming such a file re-reads it and re-reports it: two
    /// identical warnings, or the same file listed twice as excluded.
    #[test]
    fn a_file_left_out_of_the_build_is_left_out_once_per_file_not_per_declaration() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cfggates");
        // `inner_gated.rs` is `#![cfg(windows)]` and named by two declarations;
        // this matrix is the one that rules it out.
        let config = crate::config::Config::load(&fixture.join("linux-only.toml"))
            .expect("the fixture config parses");
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(
            &fixture.join("src/lib.rs"),
            config.ignore(),
            &gates,
            &mut warnings,
        );
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let gated = normalize(&fixture.join("src/inner_gated.rs"));
        assert_eq!(
            resolved.excluded.iter().filter(|p| **p == gated).count(),
            1,
            "one file, one exclusion: {:?}",
            resolved.excluded
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

    #[test]
    fn path_attr_reads_string_literal() {
        let file: syn::File = syn::parse_str("#[path = \"other.rs\"]\nmod x;").unwrap();
        let syn::Item::Mod(m) = &file.items[0] else {
            panic!("expected mod item");
        };
        assert_eq!(path_attr(&m.attrs).as_deref(), Some("other.rs"));
    }
}
