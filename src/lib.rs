//! Deadwood: a codebase health analyzer for Rust workspaces.
//!
//! The library entry point is [`analyze`], which discovers a workspace via
//! `cargo metadata`, resolves each package's module tree, and runs the
//! detectors that are currently implemented:
//!
//! - **Dead module files**: `.rs` files under a package's `src/` that are not
//!   reachable from any target root through `mod` declarations.
//! - **Unused public items and re-exports**: fully-`pub` items, and `pub use`
//!   re-exports, that no path anywhere in the workspace resolves to. Usage is
//!   established by resolving `use` declarations and qualified paths against
//!   a per-crate symbol table (`src/resolve.rs`), with a conservative
//!   fallback wherever resolution is not possible (`src/unused.rs`).
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
    /// A source file not reachable from any crate root via `mod` declarations.
    DeadFile,
    /// A `pub` item no resolved path in the workspace refers to.
    UnusedPubItem,
    /// A `pub use` re-export no resolved path in the workspace goes through.
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
        }
    }

    /// Every kind there is, so a caller that must handle all of them cannot
    /// quietly miss the next one.
    pub const ALL: [FindingKind; 6] = [
        FindingKind::DeadFile,
        FindingKind::UnusedPubItem,
        FindingKind::UnusedReexport,
        FindingKind::UnusedDependency,
        FindingKind::MisplacedDependency,
        FindingKind::UnsatisfiableCfg,
    ];
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
                if !package_reachable.contains(&file) && !package_excluded.contains(&file) {
                    findings.push(Finding {
                        kind: FindingKind::DeadFile,
                        // Every construction site leaves the severity at its
                        // default; `apply_config` sets the configured one for
                        // all of them at once, below.
                        severity: Severity::default(),
                        file: relative_to(&file, &meta.workspace_root),
                        line: None,
                        name: None,
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
        findings.extend(
            unused::find_unused_items(&crates, config.public_api(), &mut warnings)
                .into_iter()
                .map(|item| Finding {
                    kind: if item.reexport {
                        FindingKind::UnusedReexport
                    } else {
                        FindingKind::UnusedPubItem
                    },
                    severity: Severity::default(),
                    file: relative_to(&item.file, &meta.workspace_root),
                    line: Some(item.line),
                    message: if item.reexport {
                        format!(
                            "`pub use` re-export of `{}` is never referenced through this module",
                            item.name
                        )
                    } else {
                        format!(
                            "pub {} `{}` is never referenced by any resolved path in this workspace",
                            item.kind, item.name
                        )
                    },
                    name: Some(item.name),
                }),
        );
    } else {
        warnings.push(
            "unused-pub check skipped: module resolution was incomplete (see warnings above)"
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
                message: format!(
                    "{} `{}` {}, so it belongs in `{}` rather than `{}`",
                    dependency_noun(entry.declared),
                    entry.name,
                    misplacement_evidence(entry.declared, entry.belongs_in, &package.name),
                    dependency_table(entry.belongs_in),
                    dependency_table(entry.declared),
                ),
                name: Some(entry.name),
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
    let (findings, baseline) =
        baseline::run(baseline_mode, &location, findings, &meta.workspace_root)?;

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

fn relative_to(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
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
            };
            assert_eq!(FindingKind::ALL[position], kind);
            assert!(seen.insert(kind), "{} is listed twice", kind.label());
        }
        assert_eq!(seen.len(), FindingKind::ALL.len());
    }
}
