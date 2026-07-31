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
//! # `include!`, which is not a `mod`
//!
//! `include!("Windows/mod.rs")` splices a file's tokens into the item it is
//! written in. The file is compiled, so it is not dead — but it is not a
//! module either, and the two questions a caller can ask about it have
//! different answers:
//!
//! - **"Was this file reached?"** — the question [`crate::analyze_with`]'s
//!   dead-file check asks. Yes: an `include!` this walk could read names it.
//! - **"What module are its items in?"** — the question every other detector
//!   asks. The *including* module's, because the tokens land there:
//!   `include!("Windows/mod.rs")` written at the crate root puts
//!   `pub mod Wdk;` at `crate::Wdk`, not `crate::Windows::Wdk`.
//!
//! Those two answers part company again for the file's *children*. A `mod`
//! declared inside an included file resolves beside **that file**, whatever it
//! is named — `include!("a/gen.rs")` with `pub mod b;` inside it needs
//! `src/a/b.rs`, not `src/b.rs` and not `src/a/gen/b.rs` — so an included file
//! owns its directory the way `mod.rs` does, while its module path is the
//! includer's. [`child_base`] decides the first; [`Pending::module`] carries
//! the second.
//!
//! Files reached through an `include!` are returned in [`Resolved::spliced`],
//! apart from [`Resolved::files`], and the caller admits them to two things:
//! the dead-file check, which they are spared by, and the *reference* half of
//! resolution, which reads the paths they write. Their items stay out. They
//! are parsed, gated and given their module paths all the same, so the
//! boundary is one filter rather than a missing answer — but admitting a
//! generated API surface's items to resolution is a finding population of its
//! own, and it is not this phase's to create.
//!
//! The two halves separate because being wrong about them costs opposite
//! things. A definition admitted at a module path we guessed invents claims
//! about items nothing can name; a reference resolved from the wrong scope
//! only ever marks *something* reached, so the worst it does is lose a
//! finding. Dropping the references is what invents — see the note on macro
//! token streams below. A file both an `include!` and a `mod` chain reach is
//! fully *analyzed*: the `mod` walk is drained before any `include!` target is
//! followed, so the ordinary route wins.
//!
//! An `include!` whose path only a build knows —
//! `include!(concat!(env!("OUT_DIR"), ...))` — is left alone here, silently.
//! That is not a second policy for it: [`crate::deps`] reads the construct
//! through the same reader ([`crate::deps::included_file`]) and already
//! warns, once per check, that the package's references are incomplete. What
//! it must not do is *spare* files on the suspicion that the unread file might
//! name them, so a package Deadwood cannot follow keeps reporting exactly the
//! dead files it reports today. A warning from here would do worse than
//! nothing: an incomplete module tree skips the whole package's dead-file
//! check, turning an unreadable `include!` into silence.
//!
//! # Macro token streams, which are claims rather than resolutions
//!
//! A `mod` declaration can live where the parser cannot follow it: inside a
//! macro. tokio wraps whole subtrees in `cfg_fs! { pub mod fs; }`, serde
//! writes its module tree in a `macro_rules!` body, and `rustc_target`'s
//! `supported_targets!` builds `mod $module;` out of its input — between
//! them, some 800 live files across three workspaces that this walk once
//! reported dead ([#60]).
//!
//! Deadwood does not expand macros, so what it reads out of a token stream is
//! a *claim*: `mod` is a keyword and can be nothing else in any stream, but
//! the macro may discard it, rewrite it, or never be invoked. Claims are
//! therefore used in the one direction the tenets allow — to *spare* files,
//! never to warn, and never to report. Three shapes are read
//! ([`scan_token_mods`]):
//!
//! - a literal `mod name` in an invocation's arguments, resolved at the
//!   invocation site;
//! - a literal `mod name` in a `macro_rules!` body — `#[path]` attributes
//!   included, read through `#[cfg_attr]` without evaluating the condition,
//!   which makes such a declaration a claim on *two* files — resolved at the
//!   definition site and again at every invocation site
//!   ([`MacroScan::emitting`]);
//! - the bare idents of an invocation whose macro's rules say `mod $x`,
//!   probed under the inline-module prefix the rules wrap them in.
//!
//! Definitions and invocations need not share a file, so invocations are held
//! until the walk has nothing else to do and re-checked each round
//! ([`MacroScan::invocations`]). Everything queued this way lands in
//! [`Resolved::spliced`], and inherits exactly the `include!` boundary: the
//! file is spared from the dead-file check and the paths it writes are
//! resolved, while its items are not admitted — the module path a macro gives
//! its items is unknowable without expansion, so admitting them would trade
//! one invented finding for another.
//!
//! Reading the paths is not a softening of that rule but the same rule applied
//! to the other half. bun declares whole subsystems inside `cfg_jsc! { ... }`,
//! and while those files were spared correctly, every item they were the sole
//! caller of was reported unused — a claim invented out of code that was read
//! and then discarded. A path is safe to be wrong about in a way a definition
//! is not: resolving one can only mark definitions reached, so a path the
//! macro turns out to throw away costs a finding and can never manufacture
//! one. Because the items were never collected, every path in such a file is
//! attributed to [`crate::resolve`]'s "counts on its own" referrer rather than
//! to an enclosing definition the reachability walk could never reach.
//!
//! [#60]: https://github.com/rlorenzo/deadwood/issues/60
//!
//! Known simplifications, tracked for later:
//! - `#[path]` is resolved relative to the declaring file's directory, which
//!   matches rustc for the common cases but not every inline-module corner.
//! - An `include!` is followed from item position only, so one written inside a
//!   function body is not. [`crate::deps`] reads those, because a mention of a
//!   crate counts wherever it is written; a `mod` declaration spliced into a
//!   function body is not a module of the crate.
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

use proc_macro2::{Delimiter, TokenStream, TokenTree};

use crate::cfg::{Gates, Site};
use crate::config::Ignore;

/// What module resolution found from one crate root.
pub struct Resolved {
    /// Every file reached by `mod` declarations, in the analyzed build.
    pub files: Vec<ParsedFile>,
    /// Files reached only by following an `include!` — the file a readable
    /// `include!` names, and everything a `mod` chain from it reaches.
    ///
    /// They are compiled by the build, so they are not dead; that is the only
    /// claim this list is used for. Everything else about them is resolved
    /// exactly as for [`Resolved::files`] — module paths included, on the
    /// including module's basis — so moving the boundary is a matter of
    /// joining the two lists rather than computing something new. See the
    /// module docs for why it has not been moved.
    pub spliced: Vec<ParsedFile>,
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
    /// Whether the file's child modules resolve from its parent directory
    /// (`lib.rs`/`mod.rs`, every `include!` target, and every `#[path]`
    /// target) or from a stem-named directory below it.
    is_mod_root: bool,
    /// Whether the directory its children resolve from is the file's *alone*.
    ///
    /// True for a crate root, a `mod.rs`, and an ordinary `name.rs` — the
    /// first two own the directory they sit in and the third owns `name/`, and
    /// in every case nothing else puts children there. False for a `#[path]`
    /// target and an `include!` target, which resolve their children from a
    /// directory the file that declared them is already using.
    ///
    /// The distinction is only asked for when the file turns out not to be
    /// part of the build: everything below a `mod.rs` leaves the build with
    /// it, whereas sweeping the directory a `#[path]` target merely borrows
    /// would take its live neighbours too — for `#[path = "body.rs"] mod
    /// body;` in `src/lib.rs`, the whole crate.
    owns_dir: bool,
    module: Vec<String>,
    /// Whether the declaration that queued this file — and every declaration
    /// above it — confines it to a test build.
    test_only: bool,
    /// How many `include!`s were followed to get here, so that a chain of them
    /// is bounded the same way [`crate::deps`] bounds it. A `mod` declaration
    /// inherits the count rather than adding to it: it is `include!` nesting
    /// that both readers cap, and reading one crate to two different depths is
    /// how the two of them would come to disagree about the same file.
    include_depth: usize,
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
///
/// `include!` targets are followed too, into [`Resolved::spliced`], and the
/// order they are followed in is the answer to a file both routes reach: the
/// `mod` queue is drained to nothing before the first `include!` target is
/// popped, so such a file lands among the files that are analyzed rather than
/// among the ones that are only counted reachable.
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
        spliced: Vec::new(),
        excluded: Vec::new(),
    };
    let mut queue: Vec<Pending> = vec![Pending {
        path: root.to_path_buf(),
        is_mod_root: true,
        owns_dir: true,
        module: Vec::new(),
        test_only: false,
        include_depth: 0,
    }];
    // `include!` targets found by the pass being drained, held back until it
    // has drained. Where in `resolved.files` the `mod` walk stopped and the
    // spliced files begin — set when the first pass runs out, so it is the
    // count of files no `include!` was needed to reach.
    let mut deferred: Vec<Pending> = Vec::new();
    let mut spliced_from: Option<usize> = None;
    let mut macros = MacroScan::default();

    loop {
        while let Some(Pending {
            path,
            is_mod_root,
            owns_dir,
            module,
            test_only,
            include_depth,
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
                        include_depth,
                    };
                    // The subtree below was already excluded and warned about on
                    // the first walk, and repeating either would double it up —
                    // both are `Vec`s nothing dedups. The two queues are not:
                    // they are drained through the check above, which is why the
                    // re-walk re-queues `mod` children to lift their
                    // confinement, and why `include!` targets go to the real
                    // `deferred` for exactly the same reason. Dropping them here
                    // would leave a spliced file holding the `test_only` its
                    // includer no longer has.
                    collect_mod_decls(
                        &ast.items,
                        &declaring,
                        Under {
                            base: &child_base(&path, is_mod_root),
                            path_base: declaring.dir,
                            module: &file.module,
                            test_only: false,
                        },
                        &mut Walk {
                            queue: &mut queue,
                            deferred: &mut deferred,
                            excluded: &mut Vec::new(),
                            warnings: &mut Vec::new(),
                            inline_mods: &mut inline_mods,
                            // A scratch: the first walk of this file already
                            // recorded its macro claims, and recording them
                            // again would probe every invocation twice.
                            macros: &mut MacroScan::default(),
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
                    // `child_base` is where this file's children *resolve*
                    // from, which is only what they leave with when the file
                    // owns it. A `#[path]` target resolves them from a
                    // directory it shares with whoever declared it, so
                    // sweeping `child_base` there would exclude live
                    // neighbours — and an excluded file is withheld from the
                    // dependency check too, which turns a lost dead-file
                    // finding into an invented unused-dependency one. Fall
                    // back to the bounded stem-named directory, the same half
                    // answer `exclude_subtree` settles for and for the same
                    // reason.
                    let leaves_with_it = if owns_dir {
                        child_base.clone()
                    } else {
                        file_dir.join(path.file_stem().unwrap_or_default())
                    };
                    resolved.excluded.extend(rs_files_under(&leaves_with_it));
                    resolved.excluded.push(path);
                    continue;
                }
                let declaring = Declaring {
                    dir: file_dir,
                    file: &path,
                    ignore,
                    gates,
                    include_depth,
                };
                collect_mod_decls(
                    &ast.items,
                    &declaring,
                    Under {
                        base: &child_base,
                        path_base: declaring.dir,
                        module: &module,
                        test_only,
                    },
                    &mut Walk {
                        queue: &mut queue,
                        deferred: &mut deferred,
                        excluded: &mut resolved.excluded,
                        warnings,
                        inline_mods: &mut inline_mods,
                        macros: &mut macros,
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

        // The `mod` walk has run out. Everything pushed from here on was
        // reached by following an `include!` or a macro token stream, and
        // everything before it was not — which is the whole of how the two
        // lists are told apart.
        let drained = *spliced_from.get_or_insert(resolved.files.len());
        // Invocations of macros now known to declare modules have the
        // definition's claims resolved at their site: literal `mod`s from the
        // body, and their own idents under every `mod $x` prefix. Held until
        // here because a macro's definition can be parsed after its first
        // invocation, and re-checked every round because a file this round
        // spliced in can hold the definition that settles one from an
        // earlier round.
        let unsettled = std::mem::take(&mut macros.invocations);
        for invocation in unsettled {
            let Some(emission) = macros.emitting.get(&invocation.name) else {
                macros.invocations.push(invocation);
                continue;
            };
            let site = SpliceSite {
                base: invocation.base,
                dir: invocation.dir,
                module: invocation.module,
                test_only: invocation.test_only,
                include_depth: invocation.include_depth,
            };
            for declared in &emission.declared {
                let name = declared.name.clone();
                queue_speculative(&mut deferred, &site, declared, &name);
            }
            for prefix in &emission.dollar_prefixes {
                for ident in &invocation.idents {
                    let declared = TokenMod {
                        prefix: prefix.clone(),
                        path_attr: None,
                        name: ident.clone(),
                    };
                    queue_speculative(&mut deferred, &site, &declared, ident);
                }
            }
            // `#[path = $f] mod $m;` puts the file name in the invocation as a
            // string literal, and the module name beside it as an ident that
            // says nothing about the file — bun's matcher table pairs
            // `"toBeArrayOfSize.rs"` with `to_be_array_of_size`, and no rule
            // turns one into the other. So the literals are probed as `#[path]`
            // targets, under the same module name the ident probe would use
            // when there is one to pair with; the module path a speculative
            // claim carries decides nothing that a wrong guess could invent,
            // and a literal naming no file is dropped in silence like the rest.
            for prefix in &emission.dollar_path_prefixes {
                for (index, literal) in invocation.literals.iter().enumerate() {
                    let name = invocation
                        .idents
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| literal.clone());
                    let declared = TokenMod {
                        prefix: prefix.clone(),
                        path_attr: Some(literal.clone()),
                        name: name.clone(),
                    };
                    queue_speculative(&mut deferred, &site, &declared, &name);
                }
            }
        }
        if deferred.is_empty() {
            let spliced = resolved.files.split_off(drained);
            resolved.spliced = spliced;
            return resolved;
        }
        queue.append(&mut deferred);
    }
}

/// Where a file's file-backed child modules live: beside it when it owns its
/// directory (`lib.rs`, `mod.rs`, and every `include!` target), in a
/// stem-named directory otherwise.
///
/// An `include!` target owns its directory whatever it is named, which is the
/// one place this differs from the file-name rule: `include!("a/gen.rs")` with
/// `pub mod b;` inside it needs `src/a/b.rs`, where an ordinary `mod gen;`
/// leading to the same file would need `src/a/gen/b.rs`. Verified against
/// rustc rather than assumed; `an_included_files_children_live_beside_it`
/// pins both halves.
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
    /// How many `include!`s were followed to reach this file, so an `include!`
    /// written in it can be capped at [`crate::deps::MAX_INCLUDE_DEPTH`].
    include_depth: usize,
}

/// What the declarations of one file inherit from the ones above them: where
/// their files are looked for, where their `#[path]` targets are resolved
/// from, what module path their items land under, and whether they are
/// already confined to a test build.
#[derive(Clone, Copy)]
struct Under<'a> {
    base: &'a Path,
    /// Where a `#[path = "..."]` written at this level resolves from, which is
    /// *not* `base` at the top level of a file and *is* `base` inside every
    /// inline `mod` block.
    ///
    /// The reference splits the rule in two. A `#[path]` on a `mod` not inside
    /// an inline block is relative to the directory the source file is in — so
    /// `#[path = "b.rs"] mod b;` written in `src/a.rs` names `src/b.rs`, even
    /// though `mod b;` without the attribute would name `src/a/b.rs`. Inside
    /// an inline block it is relative to the directory that block's file-backed
    /// children live in, inline components included: the same attribute inside
    /// `mod inner { ... }` in `src/a.rs` names `src/a/inner/b.rs`, and in
    /// `src/lib.rs` names `src/inner/b.rs`.
    ///
    /// `base` already tracks the second of those — it starts at
    /// [`child_base`] and gains a component per inline block — so this field
    /// only has to start at the declaring file's directory and then follow it.
    path_base: &'a Path,
    module: &'a [String],
    test_only: bool,
}

/// What a walk produces: files still to load, files this build leaves out, and
/// declarations that resolved to nothing.
struct Walk<'a> {
    queue: &'a mut Vec<Pending>,
    /// `include!` targets — and every file a macro token stream declares —
    /// held back until the queue above has drained so that a file the `mod`
    /// walk also reaches is resolved as a module first.
    deferred: &'a mut Vec<Pending>,
    excluded: &'a mut Vec<PathBuf>,
    warnings: &'a mut Vec<String>,
    /// Every inline `mod` the walk passed, by module path, and whether it was
    /// confined to a test build. Reduced to [`ParsedFile::test_only_mods`] by
    /// [`confined_inline_mods`].
    inline_mods: &'a mut Vec<(Vec<String>, bool)>,
    /// What the macro token streams of the files walked so far have said —
    /// see [`MacroScan`].
    macros: &'a mut MacroScan,
}

/// What macro token streams contribute to the module tree, accumulated across
/// the whole walk because a macro's definition and its invocations need not
/// share a file — tokio's `cfg_fs!` lives in `src/macros/cfg.rs` and is
/// invoked from `src/lib.rs` ([#60]).
///
/// [#60]: https://github.com/rlorenzo/deadwood/issues/60
#[derive(Default)]
struct MacroScan {
    /// What each `macro_rules!` definition whose body declares modules would
    /// contribute at an invocation site, by macro name.
    emitting: HashMap<String, MacroEmission>,
    /// Every macro invocation passed so far that has not yet matched a name
    /// in `emitting`, kept until the walk runs out of other work: the
    /// definition that settles a macro may be parsed after its first
    /// invocation.
    invocations: Vec<MacroInvocation>,
}

/// The module declarations a macro's rules make, relative to wherever the
/// macro is invoked.
#[derive(Default)]
struct MacroEmission {
    /// Inline-module prefixes under which the rules say `mod $x` —
    /// `supported_targets!` wraps its `mod $module;` in `mod targets { .. }`,
    /// so its invocation idents are probed under `targets/`. An unwrapped
    /// `mod $x` contributes the empty prefix.
    dollar_prefixes: Vec<Vec<String>>,
    /// The same, for rules that say `#[path = $f] mod $m;` — bun declares its
    /// forty-odd Jest matcher modules with one, and the file names live in
    /// the invocation as string literals (`"toBe.rs" => to_be`) because the
    /// module names are not derivable from them. Probing the idents finds
    /// `to_be.rs`; the file is `toBe.rs`.
    dollar_path_prefixes: Vec<Vec<String>>,
    /// Literal `mod name` declarations in the rules, re-resolved at every
    /// invocation site: serde's `crate_root!` declares its whole module tree
    /// this way, `#[path]` attributes included, and the paths are relative to
    /// the file that *invokes* it.
    declared: Vec<TokenMod>,
}

/// One macro invocation and the context its candidate modules would resolve
/// in, recorded where it was written.
struct MacroInvocation {
    /// The macro's name, as the last segment of the invoked path.
    name: String,
    /// Every bare identifier in the invocation's arguments. If the macro
    /// turns out to emit `mod $x`, each is probed as a module name under the
    /// emission's prefixes; the ones that name no file are dropped without a
    /// sound.
    idents: Vec<String>,
    /// Every string literal in the invocation's arguments, probed as a
    /// `#[path]` target if the macro turns out to emit `#[path = $f] mod $m;`.
    /// Dropped without a sound when they name nothing, like the idents.
    literals: Vec<String>,
    /// Where the invocation's children would live ([`Under::base`]) and the
    /// directory `#[path]` attributes resolve from ([`Declaring::dir`]).
    base: PathBuf,
    dir: PathBuf,
    module: Vec<String>,
    test_only: bool,
    include_depth: usize,
}

/// One `mod` declaration read out of a token stream, relative to the stream's
/// expansion site.
#[derive(Clone)]
struct TokenMod {
    /// Inline modules the declaration is nested under within the stream.
    prefix: Vec<String>,
    /// The value of a `#[path = "..."]` attribute directly above it — read
    /// through `#[cfg_attr(.., path = "...")]` as well, without evaluating
    /// the condition: following a gate that does not hold spares a file, and
    /// sparing is this scan's only power.
    path_attr: Option<String>,
    name: String,
}

/// What one scan of a macro token stream found.
#[derive(Default)]
struct TokenMods {
    declared: Vec<TokenMod>,
    /// Bare identifiers, for probing when the macro emits `mod $x`.
    idents: Vec<String>,
    /// String literals, for probing when the macro emits
    /// `#[path = $f] mod $m;` — there the file names are the caller's
    /// literals, and the idents are only the module names they are bound to.
    literals: Vec<String>,
    /// The inline-module prefixes under which `mod $` was seen.
    dollar_prefixes: Vec<Vec<String>>,
    /// The inline-module prefixes under which `mod $` was seen carrying a
    /// `#[path = $..]` whose value is a metavariable too.
    dollar_path_prefixes: Vec<Vec<String>>,
}

/// Scan a macro token stream for module declarations, without expanding it.
///
/// A `mod` here is a *claim*, not a resolution: the macro may discard its
/// input, rewrite it, or never be invoked at all. Everything found is
/// therefore speculative in the one direction the tenets allow — a candidate
/// that names a real file spares it from the dead-file check, and a candidate
/// that names nothing is dropped silently, warning about nobody. `mod` cannot
/// be an identifier anywhere else in a token stream (it is a keyword), so the
/// scan does not mistake ordinary code for a declaration; what it can do is
/// read a declaration the macro would have thrown away, which loses a finding
/// rather than inventing one.
///
/// An inline `mod name { ... }` group is entered with the name pushed onto
/// `prefix`, exactly as [`collect_mod_decls`] enters the parsed form; every
/// other group — arm bodies, parentheses, attribute brackets — is entered
/// with the prefix unchanged, because the tokens inside it land wherever the
/// macro puts them and this scan's answer must not depend on that.
fn scan_token_mods(tokens: TokenStream, prefix: &[String], found: &mut TokenMods) {
    let mut iter = tokens.into_iter().peekable();
    // A `#[path = "..."]` (or `#[cfg_attr(.., path = "..")]`) seen since the
    // last declaration-shaped token, waiting for the `mod` it sits above.
    let mut pending_path: Option<String> = None;
    // The same slot for `#[path = $f]`, whose value only an invocation has.
    let mut pending_path_meta = false;
    while let Some(tree) = iter.next() {
        match tree {
            TokenTree::Ident(ident) => {
                let word = ident.to_string();
                if word != "mod" {
                    // `pub` and its `(crate)` group are the only tokens that
                    // legally stand between an attribute and its `mod`;
                    // anything else means the attribute belonged to some
                    // other item.
                    if word != "pub" {
                        pending_path = None;
                        pending_path_meta = false;
                    }
                    found.idents.push(word);
                    continue;
                }
                match iter.peek() {
                    Some(TokenTree::Ident(name)) => {
                        let name = name.to_string();
                        iter.next();
                        match iter.peek() {
                            Some(TokenTree::Group(group))
                                if group.delimiter() == Delimiter::Brace =>
                            {
                                let Some(TokenTree::Group(group)) = iter.next() else {
                                    unreachable!("peeked a group");
                                };
                                pending_path = None;
                                pending_path_meta = false;
                                let mut child_prefix = prefix.to_vec();
                                child_prefix.push(name.clone());
                                scan_token_mods(group.stream(), &child_prefix, found);
                            }
                            _ => {
                                pending_path_meta = false;
                                found.declared.push(TokenMod {
                                    prefix: prefix.to_vec(),
                                    path_attr: pending_path.take(),
                                    name,
                                });
                            }
                        }
                    }
                    Some(TokenTree::Punct(punct)) if punct.as_char() == '$' => {
                        // `#[path = $f] mod $m;` names its file from the
                        // invocation's literals, and `mod $m;` alone from its
                        // idents. Both are recorded: a rule can be read
                        // through either arm at different invocations, and a
                        // probe that names no file costs nothing.
                        let here = prefix.to_vec();
                        if pending_path_meta && !found.dollar_path_prefixes.contains(&here) {
                            found.dollar_path_prefixes.push(here.clone());
                        }
                        if !found.dollar_prefixes.contains(&here) {
                            found.dollar_prefixes.push(here);
                        }
                        pending_path_meta = false;
                    }
                    _ => {}
                }
            }
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                if let Some(TokenTree::Group(group)) = iter.peek()
                    && group.delimiter() == Delimiter::Bracket
                {
                    if let Some(literal) = path_attr_in_tokens(group.stream()) {
                        pending_path = Some(literal);
                    } else if path_attr_is_metavariable(group.stream()) {
                        pending_path_meta = true;
                    }
                    let Some(TokenTree::Group(group)) = iter.next() else {
                        unreachable!("peeked a group");
                    };
                    scan_token_mods(group.stream(), prefix, found);
                }
            }
            TokenTree::Group(group) => {
                // A parenthesis group directly after `pub` is its
                // restriction; anything else ends an attribute's reach.
                if group.delimiter() != Delimiter::Parenthesis {
                    pending_path = None;
                    pending_path_meta = false;
                }
                scan_token_mods(group.stream(), prefix, found);
            }
            TokenTree::Literal(literal) => {
                let tokens = TokenStream::from(TokenTree::Literal(literal.clone()));
                if let Ok(lit) = syn::parse2::<syn::LitStr>(tokens) {
                    found.literals.push(lit.value());
                }
            }
            _ => {}
        }
    }
}

/// The string a `path = "..."` assignment binds anywhere in an attribute's
/// tokens — which reads `#[path = ".."]` and `#[cfg_attr(cond, path = "..")]`
/// with one rule, and reads nothing into attributes that merely contain the
/// word.
fn path_attr_in_tokens(tokens: TokenStream) -> Option<String> {
    let mut iter = tokens.clone().into_iter().peekable();
    while let Some(tree) = iter.next() {
        match tree {
            TokenTree::Ident(ident) if ident == "path" => {
                if let Some(TokenTree::Punct(punct)) = iter.peek()
                    && punct.as_char() == '='
                {
                    iter.next();
                    if let Some(TokenTree::Literal(literal)) = iter.peek() {
                        // Parsed as a string literal rather than trimmed by
                        // hand, so the raw (`r"..."`) and escaped spellings
                        // read the path they mean; a non-string literal
                        // parses as nothing and claims nothing.
                        let tokens = TokenStream::from(TokenTree::Literal(literal.clone()));
                        if let Ok(lit) = syn::parse2::<syn::LitStr>(tokens) {
                            return Some(lit.value());
                        }
                    }
                }
            }
            TokenTree::Group(group) => {
                if let Some(found) = path_attr_in_tokens(group.stream()) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether an attribute's tokens bind `path` to a *metavariable* rather than
/// to a literal — `#[path = $file]`, which is a `macro_rules!` body's way of
/// saying "the caller names the file".
///
/// The declaration such a rule emits has neither a name nor a path this scan
/// can read: `#[path = $file] pub mod $mod;` puts both in the invocation. What
/// can be read is that the *invocation's string literals* are candidate file
/// paths, which is what [`MacroEmission::dollar_path_prefixes`] records.
fn path_attr_is_metavariable(tokens: TokenStream) -> bool {
    let mut iter = tokens.into_iter().peekable();
    while let Some(tree) = iter.next() {
        match tree {
            TokenTree::Ident(ident) if ident == "path" => {
                if let Some(TokenTree::Punct(punct)) = iter.peek()
                    && punct.as_char() == '='
                {
                    iter.next();
                    if let Some(TokenTree::Punct(punct)) = iter.peek()
                        && punct.as_char() == '$'
                    {
                        return true;
                    }
                }
            }
            TokenTree::Group(group) => {
                if path_attr_is_metavariable(group.stream()) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Queue the file a speculative `mod` claim names, if there is one.
///
/// The out-of-line resolution rule of [`collect_mod_decls`], minus the
/// warning: a claim read out of a token stream proves nothing when it misses,
/// so a miss is silence rather than an unresolved-module warning that would
/// skip the package's checks.
fn queue_speculative(
    deferred: &mut Vec<Pending>,
    site: &SpliceSite,
    declared: &TokenMod,
    name: &str,
) {
    let base = declared
        .prefix
        .iter()
        .fold(site.base.clone(), |base, step| base.join(step));
    // A `#[path]` read out of tokens may sit inside a `#[cfg_attr(..)]` whose
    // condition this scan never evaluates, and such a `mod` has *two*
    // possible files — the attribute's target in the builds where the
    // condition holds, the stem-named file everywhere else (serde's
    // `crate_root!` declares its whole tree this way). Both are probed and
    // every hit is queued: sparing a file is this function's only power, so
    // following a branch that does not hold costs nothing but a lost finding.
    let mut candidates = Vec::new();
    if let Some(explicit) = &declared.path_attr {
        // Two starting points, for the same reason the `cfg_attr` case above
        // has two: under an inline prefix the rules resolve a `#[path]` from
        // `base` (see `Under::path_base`), but these tokens may be spliced in
        // somewhere the prefix does not survive, and probing the declaring
        // file's directory as well only spares a file. When the prefix is
        // empty the two are the same path and the second probe finds nothing
        // new.
        for start in [&base, &site.dir] {
            let target = start.join(explicit);
            if target.is_file() && !candidates.iter().any(|(had, _, _)| had == &target) {
                // Resolves children from its parent directory whatever it is
                // named, as in `collect_mod_decls`, and owns that directory
                // only when it is literally a `mod.rs`.
                let owns_dir = target.file_name().is_some_and(|n| n == "mod.rs");
                candidates.push((target, true, owns_dir));
            }
        }
    }
    let as_file = base.join(format!("{name}.rs"));
    let as_dir = base.join(name).join("mod.rs");
    if as_file.is_file() {
        candidates.push((as_file, false, true));
    } else if as_dir.is_file() {
        candidates.push((as_dir, true, true));
    }
    for (path, is_mod_root, owns_dir) in candidates {
        let mut module = site.module.clone();
        module.extend(declared.prefix.iter().cloned());
        module.push(name.to_string());
        deferred.push(Pending {
            path,
            is_mod_root,
            owns_dir,
            module,
            test_only: site.test_only,
            include_depth: site.include_depth,
        });
    }
}

/// Where a macro's claimed modules land: the invocation (or definition) site
/// whose directories and module path they resolve against.
struct SpliceSite {
    base: PathBuf,
    dir: PathBuf,
    module: Vec<String>,
    test_only: bool,
    include_depth: usize,
}

/// Read one macro item — definition or invocation — for the module
/// declarations its tokens claim.
///
/// A `macro_rules!` definition contributes its literal `mod`s and its
/// `mod $x` prefixes to [`MacroScan::emitting`], and has the literals probed
/// at the definition site too — a macro used in the file that defines it
/// resolves the same files either way. An invocation contributes its literal
/// `mod`s (tokio's `cfg_fs! { pub mod fs; }`) immediately, and its bare
/// idents to [`MacroScan::invocations`], held until the macro's definition
/// settles what they are: `supported_targets!` passes 330 module names as
/// plain idents, and serde's `crate_root!` puts the whole module tree —
/// `#[path]` attributes included — in the macro body, to be resolved from
/// the invoking file.
fn scan_macro_item(
    mac: &syn::ItemMacro,
    declaring: &Declaring<'_>,
    under: Under<'_>,
    walk: &mut Walk<'_>,
) {
    if !declaring.gates.compiled(&mac.attrs) {
        return;
    }
    let test_only = under.test_only || declaring.gates.test_only(&mac.attrs, Site::Other);
    let mut found = TokenMods::default();
    scan_token_mods(mac.mac.tokens.clone(), &[], &mut found);
    let site = SpliceSite {
        base: under.base.to_path_buf(),
        dir: declaring.dir.to_path_buf(),
        module: under.module.to_vec(),
        test_only,
        include_depth: declaring.include_depth,
    };
    for declared in &found.declared {
        let name = declared.name.clone();
        queue_speculative(walk.deferred, &site, declared, &name);
    }
    // A stream that both declares `mod $x` *and* carries the arguments for it
    // settles itself, here, without ever reaching `MacroScan`. That happens
    // when a `macro_rules!` is defined and invoked inside another macro's
    // tokens, which the item-level reader never sees as items at all: bun
    // wraps `macro_rules! matchers` and its forty-nine-entry invocation
    // together inside one `cfg_jsc! { pub mod expect { .. } }`, so neither
    // half is an `ItemMacro` and the pairing has to be read off the one stream
    // holding both. Probing costs a `stat` per candidate and spares a file
    // when it hits, which is the only thing a claim out of a token stream is
    // allowed to do.
    for prefix in &found.dollar_path_prefixes {
        for literal in &found.literals {
            // The module name is the caller's other metavariable, and pairing
            // it with the right literal means parsing the macro's own rules.
            // The file stem stands in instead: these files land in
            // `Resolved::spliced`, where the module path names nothing that
            // resolution consults, so a stem that is not the name the macro
            // binds spares exactly the same file.
            let name = Path::new(literal)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| literal.clone());
            let declared = TokenMod {
                prefix: prefix.clone(),
                path_attr: Some(literal.clone()),
                name: name.clone(),
            };
            queue_speculative(walk.deferred, &site, &declared, &name);
        }
    }
    if mac.mac.path.is_ident("macro_rules") {
        if let Some(name) = &mac.ident
            && (!found.declared.is_empty() || !found.dollar_prefixes.is_empty())
        {
            let emission = walk.macros.emitting.entry(name.to_string()).or_default();
            emission.declared.extend(found.declared);
            for prefix in found.dollar_prefixes {
                if !emission.dollar_prefixes.contains(&prefix) {
                    emission.dollar_prefixes.push(prefix);
                }
            }
            for prefix in found.dollar_path_prefixes {
                if !emission.dollar_path_prefixes.contains(&prefix) {
                    emission.dollar_path_prefixes.push(prefix);
                }
            }
        }
        return;
    }
    let Some(name) = mac.mac.path.segments.last() else {
        return;
    };
    walk.macros.invocations.push(MacroInvocation {
        name: name.ident.to_string(),
        idents: found.idents,
        literals: found.literals,
        base: site.base,
        dir: site.dir,
        module: site.module,
        test_only,
        include_depth: declaring.include_depth,
    });
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
        if let syn::Item::Macro(mac) = item {
            queue_include(mac, declaring, under, walk);
            scan_macro_item(mac, declaring, under, walk);
            continue;
        }
        let syn::Item::Mod(m) = item else { continue };
        let name = m.ident.to_string();
        // Test-confinement accumulates downward and never lifts: a module
        // inside `#[cfg(test)] mod tests` is test code whatever its own gate
        // says, so the declaration only ever adds to what it inherited. A
        // `mod` is `Site::Other`: rustc rejects `#[test]` on one outright, so
        // only its `cfg` gates can confine what it declares.
        let test_only = under.test_only || declaring.gates.test_only(&m.attrs, Site::Other);
        // A `mod` the configured matrix rules out is not part of this build:
        // neither it nor the files under it are read, and neither is dead.
        if !declaring.gates.compiled(&m.attrs) {
            let named = path_attr(&m.attrs).map(|explicit| under.path_base.join(explicit));
            exclude_subtree(named.as_deref(), under.base, &name, walk.excluded);
            continue;
        }
        let mut child_module = under.module.to_vec();
        child_module.push(name.clone());
        match &m.content {
            // Inline module: its own file-backed children live one directory
            // level deeper (`mod a { mod b; }` in lib.rs -> src/a/b.rs).
            Some((_, inner)) => {
                // A `#[path]` on an inline `mod` names the directory its
                // file-backed children live in, replacing the name-derived one
                // — `#[path = "builtin"] mod builtins { mod ls; }` puts `ls` in
                // `builtin/ls.rs`, not `builtins/ls.rs`. It is itself a
                // `#[path]` written at this level, so it resolves from
                // `path_base` like any other, which for an inline block at the
                // top of `a.rs` is `a.rs`'s own directory and not `a/`.
                let nested_base = match path_attr(&m.attrs) {
                    Some(explicit) => under.path_base.join(explicit),
                    None => under.base.join(&name),
                };
                walk.inline_mods.push((child_module.clone(), test_only));
                collect_mod_decls(
                    inner,
                    declaring,
                    Under {
                        base: &nested_base,
                        // Inside the block, `#[path]` and the stem-named
                        // lookup agree on where to start: see `Under`.
                        path_base: &nested_base,
                        module: &child_module,
                        test_only,
                    },
                    walk,
                );
            }
            // External module: find the file it refers to.
            None => {
                let named = path_attrs(&m.attrs);
                if let Some(first) = named.first() {
                    let targets: Vec<PathBuf> = named
                        .iter()
                        .map(|explicit| under.path_base.join(explicit))
                        .collect();
                    // The first that exists is the module; the rest are the
                    // other arms of a `cfg_attr` pair, which no build compiles
                    // alongside it. They are spared rather than analyzed —
                    // attributing two files to one module path would report
                    // whichever is not in this build.
                    let target = targets.iter().find(|target| target.is_file()).cloned();
                    for other in &targets {
                        if Some(other) != target.as_ref() {
                            exclude_subtree(Some(other), under.base, &name, walk.excluded);
                        }
                    }
                    let target = target.unwrap_or_else(|| under.path_base.join(first));
                    if target.is_file() {
                        // A `#[path]` target owns its parent directory whatever
                        // it is called — rustc treats every one of them as
                        // though it were a `mod.rs`. So `#[path = "body.rs"]
                        // mod body;` in `src/lib.rs` puts `mod child;` written
                        // inside `body.rs` at `src/child.rs`, a *sibling*, and
                        // not at `src/body/child.rs` the way an ordinary
                        // `src/body.rs` would. Verified against rustc, which
                        // rejects the stem-named spelling outright.
                        // Children resolve from the parent directory either
                        // way; only a target literally named `mod.rs` owns it,
                        // rather than sharing it with the declaring file.
                        let owns_dir = target.file_name().is_some_and(|n| n == "mod.rs");
                        walk.queue.push(Pending {
                            path: target,
                            is_mod_root: true,
                            owns_dir,
                            module: child_module,
                            test_only,
                            include_depth: declaring.include_depth,
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
                        owns_dir: true,
                        module: child_module,
                        test_only,
                        include_depth: declaring.include_depth,
                    });
                } else if as_dir.is_file() {
                    walk.queue.push(Pending {
                        path: as_dir,
                        is_mod_root: true,
                        owns_dir: true,
                        module: child_module,
                        test_only,
                        include_depth: declaring.include_depth,
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

/// Queue the file an `include!` splices in, if this item is one and its path
/// is one we can read.
///
/// Three things are decided here, and each is the opposite of what a `mod`
/// declaration gets:
///
/// - The path is relative to the **declaring file's** directory, not to
///   `under.base`, which for an `include!` inside an inline `mod` is a
///   directory deeper.
/// - The module path is the **including** item's, unchanged: the tokens are
///   spliced into it, so nothing new is named.
/// - The file **owns its directory** for its own `mod` declarations whatever
///   it is called, which is [`child_base`]'s `is_mod_root` and is why an
///   `include!` target is queued with it set.
///
/// An `include!` the matrix rules out is not followed, and — unlike a `mod` it
/// rules out — takes nothing with it into [`Resolved::excluded`]: the files it
/// would have reached are its own directory's, and for `include!("gen.rs")` at
/// a crate root that directory is the whole of `src/`. Excluding it would
/// suppress every genuinely dead file beside it, which is a worse failure than
/// the one it fixes; the files fall back to the answer they get today.
fn queue_include(
    mac: &syn::ItemMacro,
    declaring: &Declaring<'_>,
    under: Under<'_>,
    walk: &mut Walk<'_>,
) {
    let Some(included) = crate::deps::included_file(&mac.mac) else {
        return;
    };
    // A path only a build knows is [`crate::deps`]'s to warn about, and this
    // walk's to leave exactly as it found it — see the module docs.
    let crate::deps::Included::At(literal) = included else {
        return;
    };
    if !declaring.gates.compiled(&mac.attrs) {
        return;
    }
    // Deeper than the reader in [`crate::deps`] follows. Stopping here leaves
    // the rest of the chain unreached, and so reported dead: a file is spared
    // only by an `include!` that was actually read.
    if declaring.include_depth >= crate::deps::MAX_INCLUDE_DEPTH {
        return;
    }
    walk.deferred.push(Pending {
        path: declaring.dir.join(literal),
        is_mod_root: true,
        owns_dir: false,
        module: under.module.to_vec(),
        // A macro invocation is `Site::Other`: rustc warns about `#[test]`
        // written on one and compiles the spliced code anyway, so a test
        // attribute here confines nothing.
        test_only: under.test_only || declaring.gates.test_only(&mac.attrs, Site::Other),
        include_depth: declaring.include_depth + 1,
    });
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
            // Where the children *resolve* from is `parent` for every
            // `#[path]` target, but what leaves the build with this module is
            // only the directory it owns — the `Pending::owns_dir` split.
            //
            // A target literally named `mod.rs` owns `parent`, so `#[path =
            // "sub/mod.rs"]` takes all of `sub/` with it. Any other target
            // shares `parent` with the file that declared it, and sweeping it
            // would exclude live neighbours: for `#[path = "body.rs"] mod
            // body;` in `src/lib.rs`, the whole crate. That is not a
            // conservative direction to err in either, because an excluded
            // file is withheld from the dependency check as well as the
            // dead-file one — over-reaching here would not merely lose a
            // finding, it would invent an unused-dependency claim by hiding
            // the mention that answers it.
            //
            // So the shared case falls back to the bounded stem-named
            // directory the module would have owned had it been declared
            // without the attribute. Siblings that only the excluded module
            // declares are not covered by that and can still be reported dead
            // — the one direction of this function that is not conservative,
            // and the reason it is kept to gates that hold in no build at all.
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

/// Every file a `mod`'s attributes name, in the order they are written.
///
/// Usually none or one. More than one comes from `#[cfg_attr(cond, path =
/// "..")]`, whose condition this function does not evaluate — the same reading
/// [`path_attr_in_tokens`] already gives a `#[path]` inside a macro, and for
/// the same reason: a gate that does not hold spares a file, and sparing is
/// the safe direction. serde declares its `internals` module with a pair of
/// them, one path per build, and reading neither left the module unresolved
/// and the whole package's checks skipped:
///
/// ```ignore
/// #[cfg_attr(serde_build_from_git, path = "../serde_derive/src/internals/mod.rs")]
/// #[cfg_attr(not(serde_build_from_git), path = "src/mod.rs")]
/// mod internals;
/// ```
///
/// The caller resolves the module to the first candidate that is a file and
/// spares the rest from the dead-file check: no build compiles more than one
/// of them, so analyzing more than one would attribute two files to a single
/// module path and report whichever is not in this build.
fn path_attrs(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut found = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("path") {
            if let syn::Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(lit) = &nv.value
                && let syn::Lit::Str(s) = &lit.lit
            {
                found.push(s.value());
            }
        } else if attr.path().is_ident("cfg_attr")
            && let syn::Meta::List(list) = &attr.meta
            && let Some(value) = path_attr_in_tokens(list.tokens.clone())
        {
            found.push(value);
        }
    }
    found
}

/// The first file a `mod`'s attributes name, which is the one it resolves to.
fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    path_attrs(attrs).into_iter().next()
}

/// All `.rs` files under `dir`, recursively, skipping hidden directories.
pub fn rs_files_under(dir: &Path) -> Vec<PathBuf> {
    rs_files_under_pruned(dir, &|_| false)
}

/// [`rs_files_under`], not descending into a directory `prune` rejects.
///
/// `prune` is asked about directories only, and only about ones below `dir` —
/// the root is walked whatever it says, so a caller cannot prune away the
/// thing it asked about.
pub fn rs_files_under_pruned(dir: &Path, prune: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
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
                if !name.starts_with('.') && !prune(&path) {
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

    /// Where a `#[path]` resolves from, for the three shapes a name-derived
    /// guess gets wrong. Every layout the fixture uses is one rustc accepts
    /// and every alternative spelling is one it rejects, so this is the rule
    /// rather than a preference — see the fixture's manifest.
    ///
    /// Found by running the analyzer over bun's Rust tree, which spells its
    /// modules this way about eight hundred times: resolution missed a hundred
    /// and sixty files, and the unreachable-module warnings that produced took
    /// the unused-pub check for the whole workspace down with them.
    #[test]
    fn a_path_attribute_resolves_from_the_inline_block_it_sits_in() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inlinepath/src/lib.rs");
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let src = root.parent().expect("the fixture root sits in `src/`");
        let mut reached: Vec<String> = resolved
            .files
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(src)
                    .expect("every reached file is under `src/`")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        reached.sort();
        assert_eq!(
            reached,
            vec![
                // A `#[path]` target owns the directory it sits in whatever it
                // is called, so `body.rs` declares siblings.
                "body.rs",
                // `#[path = "builtin"]` renames the block's directory, and
                // resolves from `src/` rather than from `src/builtins/`.
                "builtin/Ls.rs",
                // The stem-named lookup follows the renamed directory too.
                "builtin/cat.rs",
                "lib.rs",
                // A renamed block inside a renamed block: each level resolves
                // from the one above it.
                "nested/printer/Tree.rs",
                // No attribute on the block, so `#[path]` inside it resolves
                // from `src/plain/`, not from `src/`.
                "plain/Renamed.rs",
                "sibling.rs",
            ],
            "a `#[path]` must resolve from the directory its own block nests in",
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
                // Declared by an ungated `mod crossfile;`, and named there so
                // the crate-root rename in `lib.rs` does not reach it.
                ("crossfile.rs".to_string(), false),
                // The child of `shared_view.rs`, carrying no gate of its own.
                // It sits beside its parent rather than under it because
                // `shared_view.rs` is reached through `#[path]`, which resolves
                // children from the directory the target sits in.
                ("deeper.rs".to_string(), false),
                ("lib.rs".to_string(), false),
                // Its only declaration is `#[cfg(test)] mod outline_tests;`.
                ("outline_tests.rs".to_string(), true),
                // Reached by a declaration that does not confine it, among
                // ones that do.
                ("shared_view.rs".to_string(), false),
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

    /// The two questions this module asks [`Gates::test_only`] are both
    /// [`Site::Other`], and that is not a formality: a test attribute written
    /// on a `mod` declaration or on a macro invocation confines nothing.
    /// Verified against rustc — `#[test] mod tests { .. }` is `error: the
    /// #[test] attribute may only be used on a free function`, and on an
    /// `include!` it is the same message as a *warning* with the spliced code
    /// compiled into the library anyway. Neither shape belongs in a committed
    /// fixture for that reason, so the tree is written here.
    #[test]
    fn a_test_attribute_on_a_mod_declaration_or_an_include_confines_nothing() {
        let dir = std::env::temp_dir().join(format!(
            "deadwood-modtree-test-attr-{}/src",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("lib.rs"),
            "#[test]\nmod child;\n#[test]\ninclude!(\"spliced.rs\");\n",
        )
        .unwrap();
        std::fs::write(dir.join("child.rs"), "pub fn thing() {}\n").unwrap();
        std::fs::write(dir.join("spliced.rs"), "pub fn other() {}\n").unwrap();

        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&dir.join("lib.rs"), config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let mut reached: Vec<(String, bool)> = resolved
            .files
            .iter()
            .chain(&resolved.spliced)
            .map(|file| {
                let name = file.path.file_name().unwrap().to_string_lossy().to_string();
                (name, file.test_only)
            })
            .collect();
        reached.sort();
        assert_eq!(
            reached,
            vec![
                ("child.rs".to_string(), false),
                ("lib.rs".to_string(), false),
                // The spliced file, whose `test_only` phase 18 pinned and
                // nothing reads — for the same reason it pinned it.
                ("spliced.rs".to_string(), false),
            ],
            "only a `cfg` gate can confine a file"
        );
    }

    /// Write a `src/` tree into a fresh temp directory and resolve it,
    /// returning the reached file names — [`Resolved::files`] and
    /// [`Resolved::spliced`] together, since sparing from the dead-file check
    /// is the one claim the macro scan makes.
    fn reached(label: &str, files: &[(&str, &str)]) -> Vec<String> {
        let root =
            std::env::temp_dir().join(format!("deadwood-modtree-{label}-{}", std::process::id()));
        // Self-healing against a panicked earlier run: a stale tree under the
        // same pid would add files the assertions never wrote.
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("src");
        for (name, source) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, source).unwrap();
        }
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&dir.join("lib.rs"), config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let mut names: Vec<String> = resolved
            .files
            .iter()
            .chain(&resolved.spliced)
            .map(|file| {
                file.path
                    .strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    /// The tokio shape [#60] filed 381 times: `cfg_fs! { pub mod fs; }` — a
    /// literal `mod` in a macro invocation's arguments. The macro need not
    /// even be defined for the claim to spare the file; a genuinely dead file
    /// beside it stays dead.
    ///
    /// [#60]: https://github.com/rlorenzo/deadwood/issues/60
    #[test]
    fn a_mod_declared_inside_a_macro_invocation_spares_its_file() {
        let names = reached(
            "invocation-mod",
            &[
                ("lib.rs", "cfg_x! { pub mod hidden; }\n"),
                ("hidden.rs", "pub fn thing() {}\n"),
                ("orphan.rs", "pub fn stranded() {}\n"),
            ],
        );
        assert_eq!(names, ["hidden.rs", "lib.rs"], "orphan.rs stays dead");
    }

    /// The serde shape: the module tree lives as literal `mod`s inside a
    /// `macro_rules!` body defined in one file and invoked from another, so
    /// the declarations resolve at the invocation site — which is parsed
    /// *before* the definition here, pinning the held-until-settled ordering.
    #[test]
    fn a_macro_definitions_literal_mods_resolve_at_its_invocation_sites() {
        let names = reached(
            "definition-mods",
            &[
                ("lib.rs", "#[macro_use]\nmod machinery;\ntree!();\n"),
                (
                    "machinery.rs",
                    "macro_rules! tree {\n    () => {\n        mod tucked;\n    };\n}\n",
                ),
                ("tucked.rs", "pub fn thing() {}\n"),
            ],
        );
        assert_eq!(names, ["lib.rs", "machinery.rs", "tucked.rs"]);
    }

    /// The `supported_targets!` shape, 330 files of `rustc_target`: the rules
    /// say `mod $m` under an inline `mod grouped { .. }`, so the invocation's
    /// idents are probed under `grouped/` — and only the idents actually
    /// passed, so a decoy file in the same directory stays dead.
    #[test]
    fn an_emitting_macros_invocation_idents_are_probed_under_its_prefixes() {
        let names = reached(
            "emitting-idents",
            &[
                (
                    "lib.rs",
                    "#[macro_use]\nmod machinery;\nemit_mods!(alpha, beta);\n",
                ),
                (
                    "machinery.rs",
                    "macro_rules! emit_mods {\n    ($($m:ident),*) => {\n        mod grouped { $(pub mod $m;)* }\n    };\n}\n",
                ),
                ("grouped/alpha.rs", "pub fn a() {}\n"),
                ("grouped/beta.rs", "pub fn b() {}\n"),
                ("grouped/gamma.rs", "pub fn dead() {}\n"),
            ],
        );
        assert_eq!(
            names,
            [
                "grouped/alpha.rs",
                "grouped/beta.rs",
                "lib.rs",
                "machinery.rs"
            ],
            "gamma.rs was passed to nothing and stays dead"
        );
    }

    /// The `declare_passes!` shape of `rustc_mir_transform`: the invocation
    /// writes `mod abort_unwinding_calls : AbortUnwindingCalls;` — a `mod`
    /// with tokens between the name and the semicolon that only the macro
    /// understands. The name is the claim; what follows it is the macro's
    /// business.
    #[test]
    fn a_mod_with_trailing_tokens_inside_an_invocation_still_counts() {
        let names = reached(
            "trailing-tokens",
            &[
                ("lib.rs", "declare! { mod lowered : Lowered; }\n"),
                ("lowered.rs", "pub struct Lowered;\n"),
            ],
        );
        assert_eq!(names, ["lib.rs", "lowered.rs"]);
    }

    /// The boundary that keeps the scan from gutting the check: a macro whose
    /// rules declare no `mod` gets no ident probing, so passing `ghost` to an
    /// ordinary macro does not spare `ghost.rs`.
    #[test]
    fn a_quiet_macros_idents_are_not_probed() {
        let names = reached(
            "quiet-idents",
            &[
                ("lib.rs", "#[macro_use]\nmod machinery;\nquiet!(ghost);\n"),
                (
                    "machinery.rs",
                    "macro_rules! quiet {\n    ($x:ident) => {\n        fn $x() {}\n    };\n}\n",
                ),
                ("ghost.rs", "pub fn dead() {}\n"),
            ],
        );
        assert_eq!(names, ["lib.rs", "machinery.rs"], "ghost.rs stays dead");
    }

    /// A `#[cfg_attr(cond, path = "...")] mod` inside a macro body has two
    /// possible files — the attribute's target where the condition holds and
    /// the stem-named file everywhere else — and the scan never evaluates the
    /// condition, so both are spared.
    #[test]
    fn a_cfg_attr_path_mod_in_a_macro_body_spares_both_files() {
        let names = reached(
            "cfg-attr-path",
            &[
                (
                    "lib.rs",
                    // The raw-string spelling on purpose: the literal is
                    // parsed, not trimmed, so `r"..."` reads the path it
                    // means.
                    "wrap! {\n    #[cfg_attr(feature = \"alt\", path = r\"alt/actual.rs\")]\n    mod plain;\n}\n",
                ),
                ("plain.rs", "pub fn stem() {}\n"),
                ("alt/actual.rs", "pub fn attributed() {}\n"),
            ],
        );
        assert_eq!(names, ["alt/actual.rs", "lib.rs", "plain.rs"]);
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

    /// One file, as `(path relative to `src/`, module path)`.
    type Placed = (String, Vec<String>);

    /// Resolve the `included` fixture and return the two lists it produces,
    /// each sorted — the two answers an `include!` has, side by side.
    fn included_fixture() -> (Vec<Placed>, Vec<Placed>) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/included/src/lib.rs");
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let src = root.parent().unwrap().to_path_buf();
        let listing = |files: &[ParsedFile]| {
            let mut out: Vec<Placed> = files
                .iter()
                .map(|file| {
                    let name = file
                        .path
                        .strip_prefix(&src)
                        .unwrap_or(&file.path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    (name, file.module.clone())
                })
                .collect();
            out.sort();
            out
        };
        (listing(&resolved.files), listing(&resolved.spliced))
    }

    /// A `mod` declared inside an included file resolves beside **that file**,
    /// whatever it is named — the rule [`child_base`] answers with
    /// `is_mod_root`, and the one a fix that only marked the named file
    /// reached would get wrong for every child below it.
    ///
    /// Checked against rustc rather than assumed. With `src/tree/branch.rs`
    /// removed and only `src/branch.rs` left, the fixture stops compiling with
    /// `error[E0583]: file not found for module `branch``, and with
    /// `src/tree/gen.rs` in place of `mod.rs` the answer is the same file —
    /// which is why this is not the `mod.rs` rule wearing another hat.
    #[test]
    fn an_included_files_children_live_beside_it() {
        let (_, spliced) = included_fixture();
        let paths: Vec<&str> = spliced.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "tree/branch.rs",
                "tree/branch/twig.rs",
                "tree/mod.rs",
                // Spliced into an inline module, from a path relative to the
                // *file* the `include!` is written in.
                "tree/twiglet.rs",
                // Behind `#[cfg(windows)]`, which the default matrix admits.
                "winonly/mod.rs",
            ],
            "`pub mod branch;` in `src/tree/mod.rs` is `src/tree/branch.rs`, \
             and `src/branch.rs` beside the includer is reached by nothing"
        );
    }

    /// The other half of the same file's answer, and the one that differs: an
    /// included file's items belong to the **including** module, so
    /// `pub mod branch;` spliced into the crate root is `crate::branch` and
    /// not `crate::tree::branch`. Verified against rustc, which resolves
    /// `crate::b` and rejects `crate::a::b` for the same layout.
    #[test]
    fn the_module_path_of_an_included_files_items_is_the_includers() {
        let (_, spliced) = included_fixture();
        assert_eq!(
            spliced,
            vec![
                ("tree/branch.rs".to_string(), vec!["branch".to_string()]),
                (
                    "tree/branch/twig.rs".to_string(),
                    vec!["branch".to_string(), "twig".to_string()]
                ),
                // The included file itself names no module of its own.
                ("tree/mod.rs".to_string(), Vec::new()),
                // Spliced into `mod inner { ... }`, so its items are that
                // module's — the module path is the includer's wherever the
                // includer happens to be.
                ("tree/twiglet.rs".to_string(), vec!["inner".to_string()]),
                ("winonly/mod.rs".to_string(), Vec::new()),
            ],
        );
    }

    /// A file both routes reach is *analyzed*, not merely counted reachable.
    /// The `mod` queue is drained to nothing before the first `include!`
    /// target is popped, so which list `src/dual.rs` lands in is settled by
    /// the pass order rather than by which declaration happened to pop first
    /// — and the module path it keeps is the one the `mod` walk gave it.
    #[test]
    fn a_file_both_an_include_and_a_mod_chain_reach_is_analyzed() {
        let (files, spliced) = included_fixture();
        assert_eq!(
            files,
            vec![
                ("dual.rs".to_string(), vec!["dual".to_string()]),
                ("lib.rs".to_string(), Vec::new()),
            ],
            "`src/dual.rs` is `mod dual;` in the crate root and \
             `#[path = \"../dual.rs\"] mod dual_again;` in the spliced file",
        );
        assert!(
            !spliced.iter().any(|(path, _)| path == "dual.rs"),
            "one file, one list: {spliced:?}"
        );
    }

    /// Lifting a file's test-confinement lifts it from the files that file
    /// `include!`s, the same way it lifts from the files it declares with
    /// `mod`.
    ///
    /// A re-reached file is not re-read — its subtree was already excluded and
    /// warned about — but its children are re-queued, because the queue is
    /// what carries the lifted confinement down. `include!` targets ride the
    /// second queue and had been dropped on that path, so a spliced file kept
    /// the `test_only` its includer no longer had. Nothing reads a spliced
    /// file's `test_only` today (see the module docs), so this pins the value
    /// rather than any output.
    #[test]
    fn lifting_a_files_test_confinement_lifts_it_from_what_the_file_includes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/depkinds/src/lib.rs");
        let config = crate::config::Config::default();
        let package = crate::cfg::tests_support::bare_package();
        let gates = crate::cfg::Gates::new(config.cfg(), &package);
        let mut warnings = Vec::new();
        let resolved = resolve(&root, config.ignore(), &gates, &mut warnings);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");

        let confinement = |files: &[ParsedFile], name: &str| {
            files
                .iter()
                .find(|file| file.path.ends_with(name))
                .unwrap_or_else(|| panic!("`{name}` was not reached"))
                .test_only
        };
        // The same fixture `test_confinement_follows_declarations_and_an_
        // unconfined_one_clears_it` uses, and the same three declarations: what
        // is added here is that the lifted file splices one in as well as
        // declaring one.
        assert!(
            !confinement(&resolved.files, "shared_view.rs"),
            "the unconfined declaration lifts the file itself",
        );
        assert!(
            !confinement(&resolved.files, "src/deeper.rs"),
            "and the file it declares with `mod`",
        );
        assert!(
            !confinement(&resolved.spliced, "shared_view/spliced.rs"),
            "and the file it splices in, which is the one that was dropped",
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
