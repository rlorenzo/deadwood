//! Deadwood: a codebase health analyzer for Rust workspaces.
//!
//! The library entry point is [`analyze`], which discovers a workspace via
//! `cargo metadata`, resolves each package's module tree, and runs the
//! detectors that are currently implemented:
//!
//! - **Dead module files**: `.rs` files under a package's `src/` that are not
//!   reachable from any target root through `mod` declarations, and that no
//!   `include!` Deadwood can read splices into the build either.
//! - **Unused public items and re-exports**: fully-`pub` items, and `pub use`
//!   re-exports, that nothing live in the workspace reaches. Usage is
//!   established by resolving `use` declarations and qualified paths against
//!   a per-crate symbol table (`src/resolve.rs`), with a conservative
//!   fallback wherever resolution is not possible (`src/unused.rs`); a use is
//!   then attributed to the definition it is written inside, so an item only
//!   dead code refers to is reported along with it.
//! - **Unused dependencies**: `Cargo.toml` entries whose crate name a
//!   package's code never mentions, in any target and through any channel we
//!   can see (`src/deps.rs`).
//! - **Misplaced dependencies**: `Cargo.toml` entries declared in a table the
//!   code that names them cannot see — a `[dependencies]` entry only the
//!   tests, examples and benches use, or a `[build-dependencies]` entry the
//!   build script never touches (`src/deps.rs`).
//! - **Unsatisfiable `cfg` gates**: `#[cfg(...)]` gates that can hold in no
//!   build of the package — a `mod` behind a feature its manifest does not
//!   declare is dead by construction (`src/cfg.rs`).
//!
//! What each detector reports can be tuned by a `deadwood.toml`
//! (`src/config.rs`): files to ignore, a severity per finding kind, the crates
//! and item paths that are deliberate public API, the dependency entries that
//! are load bearing without being named in code, and which builds — features,
//! targets, tests — to analyze. With no config file every setting takes the
//! value that reproduces the behavior described above, and for the `cfg`
//! matrix that value is the union of every possibility.
//!
//! A project that cannot fix everything at once records what it has today in a
//! baseline file (`src/baseline.rs`), which is subtracted last of all — after
//! the configuration, so a finding `ignore` or `severity = "off"` removed never
//! reaches it. With no baseline file, that step does nothing at all.

pub mod baseline;
pub mod cfg;
pub mod config;
pub mod metadata;
pub mod modtree;
pub mod report;

mod deps;
mod glob;
mod resolve;
mod unused;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Severity};

/// The category of a finding, used for grouping and JSON output.
///
/// The serde tags are also the keys of the config file's `[severity]` table,
/// so a new kind becomes configurable the moment it is added here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A source file no crate root reaches, through `mod` declarations or
    /// through an `include!` naming a file we can read.
    DeadFile,
    /// A `pub` item nothing live in the workspace refers to: either no
    /// resolved path names it, or every path that does is written inside
    /// something nothing reaches.
    UnusedPubItem,
    /// A `pub use` re-export nothing live in the workspace goes through.
    UnusedReexport,
    /// A `Cargo.toml` dependency the declaring package's code never names.
    UnusedDependency,
    /// A `Cargo.toml` dependency declared in a table that no code referencing
    /// it can see: a `[dependencies]` entry only tests name, or a
    /// `[build-dependencies]` entry the build script does not.
    MisplacedDependency,
    /// A `#[cfg(...)]` gate that can hold in no build of its package, so the
    /// code behind it is never compiled by anyone.
    UnsatisfiableCfg,
    /// A `pub` item the workspace reaches only through its test code: not
    /// dead, but `pub` for nobody. Off by default — see
    /// [`FindingKind::default_severity`].
    TestOnlyItem,
}

impl FindingKind {
    /// How the kind is spelled in `--json`, in the config file's `[severity]`
    /// table, and in a baseline entry — one spelling for all three.
    ///
    /// `the_label_of_every_kind_is_its_serde_tag` pins this against the derive,
    /// so a new kind cannot end up with two names.
    pub fn label(self) -> &'static str {
        match self {
            FindingKind::DeadFile => "dead_file",
            FindingKind::UnusedPubItem => "unused_pub_item",
            FindingKind::UnusedReexport => "unused_reexport",
            FindingKind::UnusedDependency => "unused_dependency",
            FindingKind::MisplacedDependency => "misplaced_dependency",
            FindingKind::UnsatisfiableCfg => "unsatisfiable_cfg",
            FindingKind::TestOnlyItem => "test_only_item",
        }
    }

    /// What this kind costs when no `[severity]` entry names it.
    ///
    /// `deny` for every kind that reports something to delete, which is what
    /// makes an absent config file a no-op for all of them. `test_only_item` is
    /// the one exception, and it is `off`: every `#[cfg(test)]` helper in every
    /// codebase is a candidate for it, so a `deny` — or even a `warn`, which
    /// prints — would fire on the first run of every project that installed
    /// Deadwood for something else. The quiet-default tenet outranks the
    /// uniformity of the table; a project that wants the answer asks for it
    /// with `test_only_item = "warn"`.
    ///
    /// The match is exhaustive on purpose: a new kind has to state its default
    /// rather than inherit one.
    pub fn default_severity(self) -> Severity {
        match self {
            FindingKind::DeadFile
            | FindingKind::UnusedPubItem
            | FindingKind::UnusedReexport
            | FindingKind::UnusedDependency
            | FindingKind::MisplacedDependency
            | FindingKind::UnsatisfiableCfg => Severity::Deny,
            FindingKind::TestOnlyItem => Severity::Off,
        }
    }

    /// Every kind there is, so a caller that must handle all of them cannot
    /// quietly miss the next one.
    pub const ALL: [FindingKind; 7] = [
        FindingKind::DeadFile,
        FindingKind::UnusedPubItem,
        FindingKind::UnusedReexport,
        FindingKind::UnusedDependency,
        FindingKind::MisplacedDependency,
        FindingKind::UnsatisfiableCfg,
        FindingKind::TestOnlyItem,
    ];
}

/// Which of Rust's namespaces a definition binds its name in.
///
/// Rust resolves a name in the type namespace and the value namespace
/// independently, which is why `pub struct Group { .. }` and
/// `#[allow(non_snake_case)] pub fn Group(..)` compile side by side in one
/// module. Nothing else on a [`Finding`] tells those two apart — same kind,
/// same file, same name, same module — so this is what a baseline entry needs
/// to name one of them without covering the other
/// ([#30](https://github.com/rlorenzo/deadwood/issues/30)).
///
/// Three values, not two, and the third is a quarter of the `pub` structs in
/// the corpus: a **unit or tuple** struct binds its name in *both* namespaces,
/// because the constructor is a value of the same name. So is a `use` alias,
/// which imports every namespace the path it names resolves in.
///
/// The macro namespace has no value here: Deadwood reports no macro
/// definitions, so no finding is ever about a name bound in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Namespace {
    /// `struct` with named fields, `enum`, `trait`, `type`, `union`, `mod`.
    Type,
    /// `fn`, `const`, `static`.
    Value,
    /// Both at once: a unit or tuple `struct`, whose constructor is a value of
    /// the same name, and a `use` alias, which binds whatever its target does.
    Both,
}

impl Namespace {
    /// Whether these two could name the same definition — whether the sets of
    /// namespaces they stand for intersect.
    ///
    /// This is the whole matching rule, and it is set overlap rather than
    /// equality because [`Namespace::Both`] is a set of two. `Both` therefore
    /// covers everything, which is the forgiving direction and the only one
    /// that is safe: a `pub struct Foo;` that gains a field becomes
    /// [`Namespace::Type`], and a baseline entry recorded against the unit
    /// spelling has to keep matching it.
    ///
    /// What that leaves uncovered is a `Both` definition beside a `Value` one
    /// in the same module — and those cannot both exist in one build, because
    /// both bind the name in the value namespace and rustc rejects the second
    /// (E0428). Deadwood analyzes the union of every `cfg` configuration, so
    /// the shape does occur here: as two `cfg`-alternative spellings of one
    /// item, which is exactly the case one baseline entry *should* cover.
    pub fn overlaps(self, other: Namespace) -> bool {
        self == other || self == Namespace::Both || other == Namespace::Both
    }

    /// How a stale baseline entry names this, for a reader looking for the
    /// definition it recorded.
    ///
    /// Two entries under one name in one module differ in nothing else the
    /// report prints, so without this the list would repeat a line and expect
    /// the reader to guess which item each one was about.
    pub fn describe(self) -> &'static str {
        match self {
            Namespace::Type => "type namespace",
            Namespace::Value => "value namespace",
            Namespace::Both => "type and value namespaces",
        }
    }
}

/// A single issue reported by the analyzer.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// How much this finding matters, from the config file's `[severity]`
    /// table. Only `deny` findings fail the run; `off` ones never reach here
    /// at all.
    pub severity: Severity,
    /// Path relative to the workspace root.
    pub file: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The module the named item is written in, `crate`-rooted:
    /// `crate::alpha`. Present for the three item kinds and absent for the four
    /// that have no module to name — a dead file has no item at all, the two
    /// dependency kinds name a manifest entry, and an unsatisfiable gate names
    /// a site rather than a definition.
    ///
    /// It is here rather than on a baseline entry alone, and that placement was
    /// the decision of the phase that added it. The baseline's format *is* this
    /// struct's serialization ([`crate::baseline`]), so a field only the entry
    /// carried would be a second format and a value no `--json` output could
    /// produce. The cost is that `--json` grows a key: a consumer that ignores
    /// unknown fields sees nothing change — every field it reads is present,
    /// unchanged, in the same order, and the finding list, its order, the counts
    /// and the exit code are all identical — while one that rejects unknown
    /// fields, or constructs this struct with a literal, has to learn about it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Which namespace the named item binds its name in, for the same three
    /// item kinds that carry a [`Finding::module`] and absent for the same four
    /// that do not — see [`Namespace`].
    ///
    /// Present on exactly the entries `module` is present on. That does not
    /// make the field free — a Deadwood that knows `module` and not this one
    /// rejects a baseline carrying it — but it does mean no *file* becomes
    /// version-sensitive that was not already ([`crate::baseline`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<Namespace>,
    pub message: String,
}

/// The result of analyzing a workspace.
#[derive(Debug, Serialize)]
pub struct Analysis {
    pub workspace_root: PathBuf,
    pub findings: Vec<Finding>,
    /// Non-fatal problems hit during analysis (unparsable files, unresolved
    /// `mod` declarations, dependency entries the configured `cfg` matrix
    /// leaves out of the build being analyzed). Whenever
    /// something could cause a detector to report false positives —
    /// incomplete module resolution for dead files, unseen definitions or
    /// paths for unused pub items, unseen code or an unevaluated gate for
    /// dependencies — that detector is skipped for the affected scope, so
    /// findings stay trustworthy but the analysis is incomplete until the
    /// warnings are resolved.
    pub warnings: Vec<String>,
    /// What the baseline file did to this run, when there was one.
    ///
    /// `None` — no `baseline` key and no file at the default location — is
    /// what keeps a project that has never adopted a baseline byte-identical
    /// to a Deadwood that has never heard of one, in the text report and in
    /// the JSON alike.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<baseline::Report>,
}

impl Analysis {
    /// Whether the run should be treated as a failure.
    ///
    /// Only `deny` findings count: a `warn` finding is printed and forgiven,
    /// which is the whole point of being able to configure one.
    pub fn has_denied(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Deny)
    }
}

/// Analyze the workspace containing `path` and return all findings.
///
/// `config_path` names a configuration file explicitly; `None` discovers one
/// by walking up from `path` to the workspace root, and falls back to
/// [`Config::default`] — which is exactly the behavior of a Deadwood with no
/// configuration support at all.
///
/// A baseline file, if the configuration names one or one sits at the default
/// location, is subtracted from the findings; see [`analyze_with`] for the
/// modes that write it instead.
pub fn analyze(path: &Path, config_path: Option<&Path>) -> Result<Analysis> {
    analyze_with(path, config_path, baseline::Mode::default())
}

/// [`analyze`], choosing what the run does with the baseline file.
///
/// [`baseline::Mode::Write`] and [`baseline::Mode::Prune`] are the only ways a
/// run creates or modifies that file; every other path here reads at most.
pub fn analyze_with(
    path: &Path,
    config_path: Option<&Path>,
    baseline_mode: baseline::Mode,
) -> Result<Analysis> {
    let meta = metadata::load(path)?;
    let config = match config_path {
        Some(explicit) => Config::load(explicit)?,
        None => Config::discover(path, &meta.workspace_root)?,
    };
    let ignore = config.ignore();
    let mut findings = Vec::new();
    let mut warnings = Vec::new();
    let mut gate_sites = GateSites::default();

    // Every target is a crate of its own for name resolution: a bin and the
    // lib it uses see different scopes, and the same file pulled into two
    // packages via `#[path]` is a separate module in each.
    let mut crates: Vec<resolve::CrateUnit> = Vec::new();
    // Package name to its library crate, so a dependency rename can be
    // attached to the crate the alias actually names.
    let mut lib_of_package: HashMap<&str, usize> = HashMap::new();
    // Crate names each package's code refers to, for the dependency check.
    // Packages whose module tree did not resolve are left out entirely.
    let mut references: Vec<(&metadata::Package, deps::CrateReferences, cfg::Gates<'_>)> =
        Vec::new();

    for package in &meta.packages {
        let manifest_dir = package
            .manifest_path
            .parent()
            .context("manifest path has no parent directory")?;
        // Features are declared per package, so which builds exist is too.
        let gates = cfg::Gates::new(config.cfg(), package);

        let warnings_before = warnings.len();
        let mut package_reachable: HashSet<PathBuf> = HashSet::new();
        // Files a `cfg` keeps out of the analyzed build: unreachable, but not
        // dead — nothing reaches them because this build does not have them.
        let mut package_excluded: HashSet<PathBuf> = HashSet::new();
        // Files an `include!` splices into the build. Compiled, so not dead —
        // and that is the whole of what they are used for here. Their items
        // are deliberately kept out of resolution and out of the dependency
        // check; see [`modtree`]'s module docs for the boundary and why it
        // sits there.
        let mut package_spliced: HashSet<PathBuf> = HashSet::new();
        let mut package_references = deps::CrateReferences::default();
        for target in &package.targets {
            let resolved = modtree::resolve(&target.src_path, ignore, &gates, &mut warnings);
            for file in &resolved.files {
                package_reachable.insert(file.path.clone());
                // Unlike the detectors below, this one is not gated on the
                // package resolving completely. Whether a gate can hold is a
                // property of one file's attributes and the manifest's feature
                // list; a sibling file that failed to parse says nothing about
                // it, and cannot turn a non-finding into a finding.
                if let Some(ast) = &file.ast {
                    gate_sites.record(&file.path, &package.name, gates.gate_sites(ast));
                }
            }
            package_spliced.extend(resolved.spliced.into_iter().map(|file| file.path));
            package_excluded.extend(resolved.excluded);
            // Every target of the package can name a dependency, including
            // its tests, examples, benches, and build script — and *which*
            // target names it is what decides whether the entry is in the
            // right table.
            package_references.add_target(&resolved.files, target, &gates);
            let names = crate_names(package, target);
            if !names.is_empty() {
                lib_of_package.insert(package.name.as_str(), crates.len());
            }
            crates.push(resolve::CrateUnit {
                names,
                // The same rule the dependency check places a mention by: a
                // test, example or bench target is code `cargo test` builds and
                // no consumer of the package runs.
                test_code: deps::is_dev_target(target),
                files: resolved.files,
            });
        }

        // An unparsable file or unresolved `mod` means the reachable set is
        // incomplete, and files it would have reached would be reported as
        // false-positive dead files — skip the check for this package.
        // A file we could not read or parse may hold the only reference to a
        // dependency, so both dependency checks are skipped for the package
        // too — the unseen file could name a crate nothing else names, and it
        // could name one from code no other target has.
        if warnings.len() > warnings_before {
            for check in ["dead-file", "unused-dependency", "misplaced-dependency"] {
                warnings.push(format!(
                    "{check} check skipped for package `{}`: module resolution was incomplete (see warnings above)",
                    package.name
                ));
            }
            continue;
        }
        // Reachability is the wrong question for a dependency: a file that no
        // `mod` declaration names can still be compiled (a macro that expands
        // to `mod`s) and can hold the only reference. A file the `cfg` matrix
        // excluded is the one exception — it is not compiled in the build
        // being analyzed, so a mention in it is not evidence about this build.
        // An `include!`-ed file is *not* already read here, and deliberately:
        // the dependency check never saw its names through the module tree,
        // because the boundary above keeps spliced files out of it. It is read
        // below like any other file no `mod` declaration names — which is what
        // happened before this file was reachable at all, so nothing the
        // dependency check sees changes.
        let mut already_read = package_reachable.clone();
        already_read.extend(package_excluded.iter().cloned());
        package_references.add_unreached_sources(manifest_dir, &already_read);
        references.push((package, package_references, gates));

        // Dead-file detection only covers src/: tests/, examples/, and
        // benches/ roots are auto-discovered targets and already covered
        // above, but stray helper files there are usually intentional.
        let src_dir = manifest_dir.join("src");
        if src_dir.is_dir() {
            for file in modtree::rs_files_under(&src_dir) {
                if !package_reachable.contains(&file)
                    && !package_excluded.contains(&file)
                    && !package_spliced.contains(&file)
                {
                    findings.push(Finding {
                        kind: FindingKind::DeadFile,
                        // Every construction site leaves the severity at its
                        // default; `apply_config` sets the configured one for
                        // all of them at once, below.
                        severity: Severity::default(),
                        file: relative_to(&file, &meta.workspace_root),
                        line: None,
                        name: None,
                        // A dead file is not an item: there is no module
                        // inside it to name, no name to bind in a namespace,
                        // and the file path is already the whole of the
                        // finding's identity.
                        module: None,
                        namespace: None,
                        message: format!(
                            "not reachable from any target of package `{}` via `mod` declarations",
                            package.name
                        ),
                    });
                }
            }
        }
    }

    add_dependency_aliases(&meta, &lib_of_package, &mut crates);

    // Usage resolution must see every file in the workspace: if module
    // resolution hit any problem, files (and the paths inside them) may be
    // missing, and unseen paths would turn into false positives.
    if warnings.is_empty() {
        let items = unused::find_items(&crates, config.public_api(), &mut warnings);
        findings.extend(items.unused.into_iter().map(|item| Finding {
            kind: if item.reexport {
                FindingKind::UnusedReexport
            } else {
                FindingKind::UnusedPubItem
            },
            severity: Severity::default(),
            file: relative_to(&item.file, &meta.workspace_root),
            line: Some(item.line),
            message: match (item.reexport, item.only_from_unreached) {
                (true, false) => format!(
                    "`pub use` re-export of `{}` is never referenced through this module",
                    item.name
                ),
                (true, true) => format!(
                    "`pub use` re-export of `{}` is referenced only from items that \
                             nothing reaches",
                    item.name
                ),
                (false, false) => format!(
                    "pub {} `{}` is never referenced by any resolved path in this workspace",
                    item.kind, item.name
                ),
                (false, true) => format!(
                    "pub {} `{}` is referenced only from items that nothing reaches",
                    item.kind, item.name
                ),
            },
            name: Some(item.name),
            module: Some(item.module),
            namespace: Some(item.namespace),
        }));
        // The narrower claim, from the same pass: reached, but only from test
        // code. It says what to *do* rather than that the item is dead —
        // "only tests reach this" is often exactly what the author wants, and
        // the fix is then a visibility, not a deletion.
        findings.extend(items.test_only.into_iter().map(|item| Finding {
            kind: FindingKind::TestOnlyItem,
            severity: Severity::default(),
            file: relative_to(&item.file, &meta.workspace_root),
            line: Some(item.line),
            message: if item.reexport {
                format!(
                    "`pub use` re-export of `{}` is reached only from test code: make it \
                     `pub(crate) use`, or move it behind `#[cfg(test)]`",
                    item.name
                )
            } else {
                format!(
                    "pub {} `{}` is reached only from test code: make it `pub(crate)`, or move \
                     it behind `#[cfg(test)]`",
                    item.kind, item.name
                )
            },
            name: Some(item.name),
            module: Some(item.module),
            namespace: Some(item.namespace),
        }));
    } else {
        // Both kinds, because both come out of the one resolution pass: a
        // reader who turned `test_only_item` on and sees nothing has to be
        // told that nothing is what a skipped check reports.
        warnings.push(
            "unused-pub and test-only checks skipped: module resolution was incomplete (see \
             warnings above)"
                .to_string(),
        );
    }

    // Last, because the warnings it raises about entries it cannot judge
    // (optional, platform-gated) say nothing about the checks above, which
    // gate themselves on the warning list being clean.
    for (package, package_references, gates) in &references {
        let manifest = relative_to(&package.manifest_path, &meta.workspace_root);
        findings.extend(
            deps::find_unused(
                package,
                package_references,
                config.dependencies(),
                gates,
                &mut warnings,
            )
            .into_iter()
            .map(|entry| Finding {
                kind: FindingKind::UnusedDependency,
                severity: Severity::default(),
                file: manifest.clone(),
                line: None,
                message: format!(
                    "{} `{}` is never referenced by any target of package `{}`",
                    dependency_noun(entry.kind),
                    entry.name,
                    package.name
                ),
                name: Some(entry.name),
                // A manifest entry is in no module, and a crate name in a
                // manifest is in no namespace of the crate's own code.
                module: None,
                namespace: None,
            }),
        );
        findings.extend(
            deps::find_misplaced(
                package,
                package_references,
                config.dependencies(),
                gates,
                &mut warnings,
            )
            .into_iter()
            .map(|entry| Finding {
                kind: FindingKind::MisplacedDependency,
                severity: Severity::default(),
                file: manifest.clone(),
                line: None,
                // A duplicate is not a move: its real use already lives in
                // the other table, and "belongs in `[dependencies]`" about an
                // entry already declared there reads as a bug.
                message: if entry.duplicate {
                    format!(
                        "{} `{}` duplicates the `{}` entry of package `{}` while enabling \
                         nothing more, and no test, example or bench code references the crate, \
                         so the `{}` copy is stale",
                        dependency_noun(entry.declared),
                        entry.name,
                        dependency_table(entry.belongs_in),
                        package.name,
                        dependency_table(entry.declared),
                    )
                } else {
                    format!(
                        "{} `{}` {}, so it belongs in `{}` rather than `{}`",
                        dependency_noun(entry.declared),
                        entry.name,
                        misplacement_evidence(entry.declared, entry.belongs_in, &package.name),
                        dependency_table(entry.belongs_in),
                        dependency_table(entry.declared),
                    )
                },
                name: Some(entry.name),
                module: None,
                namespace: None,
            }),
        );
    }

    findings.extend(gate_sites.into_findings(&meta.workspace_root));

    let mut findings = apply_config(findings, &config, &meta.workspace_root);
    findings.sort_by(|a, b| {
        (a.file.as_path(), a.line.unwrap_or(0)).cmp(&(b.file.as_path(), b.line.unwrap_or(0)))
    });

    // Last of all, and after the configuration: a finding an `ignore` pattern
    // or a `severity = "off"` removed does not exist, so it is neither
    // suppressed nor recorded — and a baseline entry for one goes stale, which
    // is the honest answer and is fixable with `--prune-baseline`.
    let location = match config.baseline() {
        Some(configured) => baseline::Location::Configured(configured.to_path_buf()),
        None => baseline::Location::Default(meta.workspace_root.join(baseline::FILE_NAME)),
    };
    // Which package a recorded path belongs to is the one thing a baseline
    // entry's `module` cannot say — it is `crate`-relative — and it is what
    // keeps a moved file from being matched across two members. Directories,
    // not files: the point is to answer for a path whose file is gone.
    let packages = baseline::Packages::new(meta.packages.iter().map(|package| {
        let directory = package
            .manifest_path
            .parent()
            .unwrap_or(&meta.workspace_root);
        (
            relative_to(directory, &meta.workspace_root),
            package.name.clone(),
        )
    }));
    let (findings, baseline) = baseline::run(
        baseline_mode,
        &location,
        findings,
        &meta.workspace_root,
        &packages,
    )?;

    Ok(Analysis {
        workspace_root: meta.workspace_root,
        findings,
        warnings,
        baseline,
    })
}

/// Every `#[cfg]` gate seen in the workspace, keyed by where it is written.
///
/// The keying is not bookkeeping, it is the correctness argument. Features are
/// declared per package, and one file can belong to several: `#[path]` pulls
/// the same source into two members, and a target's root is read once per
/// target. A gate impossible for one package may be perfectly satisfiable for
/// another, so a site is only reported when *no* package that compiles it
/// found the gate satisfiable.
#[derive(Default)]
struct GateSites {
    /// Site to the package and gate that condemned it.
    impossible: BTreeMap<(PathBuf, usize), (String, cfg::GateSite)>,
    /// Sites some package can compile, which overrule the map above.
    satisfiable: HashSet<(PathBuf, usize)>,
}

impl GateSites {
    fn record(&mut self, file: &Path, package: &str, sites: Vec<cfg::GateSite>) {
        for site in sites {
            let key = (file.to_path_buf(), site.line);
            match site.verdict {
                cfg::Verdict::CanHold => {
                    self.satisfiable.insert(key);
                }
                cfg::Verdict::Impossible { .. } => {
                    self.impossible
                        .entry(key)
                        .or_insert_with(|| (package.to_string(), site));
                }
            }
        }
    }

    fn into_findings(self, workspace_root: &Path) -> Vec<Finding> {
        self.impossible
            .into_iter()
            .filter(|(key, _)| !self.satisfiable.contains(key))
            .map(|((file, line), (package, site))| Finding {
                kind: FindingKind::UnsatisfiableCfg,
                severity: Severity::default(),
                file: relative_to(&file, workspace_root),
                line: Some(line),
                message: match &site.verdict {
                    cfg::Verdict::Impossible { undeclared } if !undeclared.is_empty() => {
                        format!(
                            "`#[cfg({})]` can never hold: package `{package}` declares no {}",
                            site.gate,
                            feature_list(undeclared)
                        )
                    }
                    _ => format!(
                        "`#[cfg({})]` can never hold in any build of package `{package}`",
                        site.gate
                    ),
                },
                name: site.name,
                // A gate site, not a definition: the name is whatever the gate
                // sits on, and nothing here tracks which module that is or
                // which namespace it binds in.
                module: None,
                namespace: None,
            })
            .collect()
    }
}

/// What a manifest entry of this kind is called in a finding message.
fn dependency_noun(kind: metadata::DependencyKind) -> &'static str {
    match kind {
        metadata::DependencyKind::Normal => "dependency",
        metadata::DependencyKind::Development => "dev-dependency",
        metadata::DependencyKind::Build => "build-dependency",
    }
}

/// The `Cargo.toml` table an entry of this kind is written in, which is what a
/// misplacement finding has to name on both sides to be actionable.
fn dependency_table(kind: metadata::DependencyKind) -> &'static str {
    match kind {
        metadata::DependencyKind::Normal => "[dependencies]",
        metadata::DependencyKind::Development => "[dev-dependencies]",
        metadata::DependencyKind::Build => "[build-dependencies]",
    }
}

/// What the references say about an entry, phrased for its finding.
///
/// The two directions are stated differently on purpose. Moving a
/// `[dependencies]` entry down is justified by where the references *are*;
/// moving a `[build-dependencies]` entry is justified by where they are not,
/// since the build script is the only thing that can use one at all.
fn misplacement_evidence(
    declared: metadata::DependencyKind,
    belongs_in: metadata::DependencyKind,
    package: &str,
) -> String {
    match (declared, belongs_in) {
        (metadata::DependencyKind::Build, _) => format!(
            "is never referenced by the build script of package `{package}`, only by its {}",
            match belongs_in {
                metadata::DependencyKind::Development => "test, example and bench code",
                // Every target kind the runtime context covers, since a
                // proc-macro crate has no other lib and would otherwise read
                // its own finding as being about a target it does not have.
                _ => "library, binaries and proc-macro code",
            }
        ),
        // A dev-dependency the library names. One such mention is enough, so
        // the phrasing says what was found rather than "only", which would be
        // false whenever the tests name it as well — as they usually do.
        //
        // The same three target kinds as the arm above, joined by "or" rather
        // than "and". There the list sits behind "only by", which reads as the
        // category it is; here it is a positive statement, and "and" would
        // claim all three referenced the entry — wrong for the lib-only
        // package that is the common case, and wrong in the direction of
        // naming targets that do not exist.
        (metadata::DependencyKind::Development, metadata::DependencyKind::Normal) => format!(
            "is referenced by the library, binary or proc-macro code of package `{package}`, \
             which cannot link a dev-dependency"
        ),
        _ => {
            format!("is referenced only by the test, example and bench code of package `{package}`")
        }
    }
}

/// `feature `x`` or `features `x`, `y``, for the finding message.
fn feature_list(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.len() {
        1 => format!("feature {}", quoted[0]),
        _ => format!("features {}", quoted.join(", ")),
    }
}

/// Drop the findings the configuration silences and label the rest with their
/// configured severity.
///
/// One pass over every finding, rather than a check inside each detector, is
/// the point: `ignore` and `[severity]` then cover a new detector by virtue of
/// it producing findings at all, with no config plumbing of its own.
///
/// Note what this does *not* do: it never suppresses evidence, only reports.
/// An ignored file has already been read, and the paths in it have already
/// marked items used, because generated code that calls a `pub fn` is still
/// calling it — dropping its references would make every `ignore` entry a
/// source of false positives somewhere else.
fn apply_config(findings: Vec<Finding>, config: &Config, workspace_root: &Path) -> Vec<Finding> {
    let ignore = config.ignore();
    findings
        .into_iter()
        .filter(|finding| !ignore.matches(&workspace_root.join(&finding.file)))
        .map(|finding| Finding {
            severity: config.severity(finding.kind),
            ..finding
        })
        .filter(|finding| finding.severity != Severity::Off)
        .collect()
}

/// Register every `foo = { package = "bar" }` rename as a name for the
/// renamed crate.
///
/// Code spells a renamed dependency by its alias, which is derivable from
/// neither the dependency's package name nor its lib target name, so without
/// this a path through the alias resolves to nothing and the items it reaches
/// look unused.
fn add_dependency_aliases(
    meta: &metadata::Metadata,
    lib_of_package: &HashMap<&str, usize>,
    crates: &mut [resolve::CrateUnit],
) {
    for package in &meta.packages {
        for dependency in &package.dependencies {
            let Some(rename) = &dependency.rename else {
                continue;
            };
            // Only workspace members are analyzed; a rename of an external
            // crate names nothing we could resolve into.
            let Some(&krate) = lib_of_package.get(dependency.name.as_str()) else {
                continue;
            };
            let alias = rename.replace('-', "_");
            if !crates[krate].names.contains(&alias) {
                crates[krate].names.push(alias);
            }
        }
    }
}

/// `path` relative to `root` for display, or `path` unchanged when it does
/// not sit under `root`.
///
/// The two can spell one place differently: a config-derived path is
/// canonicalized (the ancestor walk in [`config::Config::discover`] needs it),
/// while `cargo metadata`'s `workspace_root` keeps whatever spelling it was
/// invoked with — and on macOS the standard temp directory reaches its files
/// through a symlink (`/var` is `/private/var`), so the plain prefix strip
/// misses and the report printed an absolute path where every other line was
/// relative ([#53](https://github.com/rlorenzo/deadwood/issues/53)).
/// Canonicalizing both sides settles the spelling; a path that is still not
/// under the root after that genuinely lives elsewhere, and stays absolute.
fn relative_to(path: &Path, root: &Path) -> PathBuf {
    if let Ok(relative) = path.strip_prefix(root) {
        return relative.to_path_buf();
    }
    if let (Ok(canonical_path), Ok(canonical_root)) = (path.canonicalize(), root.canonicalize())
        && let Ok(relative) = canonical_path.strip_prefix(&canonical_root)
    {
        return relative.to_path_buf();
    }
    path.to_path_buf()
}

/// The names other crates can use for `target` in paths.
///
/// Only library targets can be named at all; cargo normalizes `-` to `_` in
/// crate names. The package name is kept as a fallback for the usual case
/// where a package does not rename its lib target.
fn crate_names(package: &metadata::Package, target: &metadata::Target) -> Vec<String> {
    const LIB_KINDS: [&str; 6] = ["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];
    if !target
        .kind
        .iter()
        .any(|kind| LIB_KINDS.contains(&kind.as_str()))
    {
        return Vec::new();
    }
    let mut names = vec![target.name.replace('-', "_")];
    let from_package = package.name.replace('-', "_");
    if !names.contains(&from_package) {
        names.push(from_package);
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The display strip must survive the root and the path spelling one
    /// place two ways. macOS hands every test a temp directory behind a
    /// symlink (`/var` is `/private/var`), which is where
    /// [#53](https://github.com/rlorenzo/deadwood/issues/53) was found — this
    /// builds its own symlink so the case exists on every platform CI runs.
    #[cfg(unix)]
    #[test]
    fn a_path_is_relative_to_a_root_spelled_through_a_symlink() {
        let scratch = std::env::temp_dir().join(format!("dw-relative-{}", std::process::id()));
        // A panicked earlier run leaves the directory behind, and a leftover
        // would fail the symlink below forever after; starting clean makes
        // the test self-healing, which a drop guard alone would not.
        let _ = std::fs::remove_dir_all(&scratch);
        let real = scratch.join("real");
        let link = scratch.join("link");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("baseline.json"), b"{}").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // The canonical path under the symlink spelling of the root, and the
        // reverse: both must come out relative.
        let canonical = real.canonicalize().unwrap().join("baseline.json");
        assert_eq!(relative_to(&canonical, &link), Path::new("baseline.json"));
        assert_eq!(
            relative_to(&link.join("baseline.json"), &real.canonicalize().unwrap()),
            Path::new("baseline.json")
        );
        // A path genuinely outside the root stays as it was.
        assert_eq!(
            relative_to(&canonical, Path::new("/nonexistent-root")),
            canonical
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// Three places spell a finding kind — `--json`, `[severity]`, and a
    /// baseline entry — and all three must be the one spelling. The label is a
    /// hand-written match, so this is what stops it drifting from the derive.
    #[test]
    fn the_label_of_every_kind_is_its_serde_tag() {
        for kind in FindingKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::from(kind.label()),
            );
        }
    }

    /// Phase 3 promised that a new kind is configurable and baselineable with
    /// no plumbing of its own, on the strength of `[severity]` defaulting
    /// uniformly. `test_only_item` is the first kind that cannot take that
    /// default — it would fire on every codebase with a `#[cfg(test)]` helper
    /// — so the promise is kept by moving the default onto the kind rather
    /// than by giving this one a switch of its own. This is what stops a later
    /// kind picking up `off` by accident, in either direction.
    #[test]
    fn test_only_item_is_the_only_kind_that_does_not_default_to_deny() {
        for kind in FindingKind::ALL {
            let expected = match kind {
                FindingKind::TestOnlyItem => Severity::Off,
                _ => Severity::Deny,
            };
            assert_eq!(
                kind.default_severity(),
                expected,
                "`{}` defaults to the wrong severity",
                kind.label()
            );
        }
    }

    /// [`FindingKind::ALL`] is what the report and the tests iterate to prove
    /// they cover every kind, so a kind missing from it would let them all
    /// pass while covering nothing. The match below is exhaustive, so a new
    /// variant does not compile until it is given a position — and the
    /// assertions then fail until it has that position in `ALL`.
    #[test]
    fn all_holds_every_finding_kind_exactly_once() {
        let mut seen = HashSet::new();
        for kind in FindingKind::ALL {
            let position = match kind {
                FindingKind::DeadFile => 0,
                FindingKind::UnusedPubItem => 1,
                FindingKind::UnusedReexport => 2,
                FindingKind::UnusedDependency => 3,
                FindingKind::MisplacedDependency => 4,
                FindingKind::UnsatisfiableCfg => 5,
                FindingKind::TestOnlyItem => 6,
            };
            assert_eq!(FindingKind::ALL[position], kind);
            assert!(seen.insert(kind), "{} is listed twice", kind.label());
        }
        assert_eq!(seen.len(), FindingKind::ALL.len());
    }
}
