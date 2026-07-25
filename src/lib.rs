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

pub mod metadata;
pub mod modtree;
pub mod report;

mod resolve;
mod unused;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// The category of a finding, used for grouping and JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A source file not reachable from any crate root via `mod` declarations.
    DeadFile,
    /// A `pub` item no resolved path in the workspace refers to.
    UnusedPubItem,
    /// A `pub use` re-export no resolved path in the workspace goes through.
    UnusedReexport,
}

/// A single issue reported by the analyzer.
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
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
    /// `mod` declarations). Whenever a warning could cause a detector to
    /// report false positives — incomplete module resolution for dead files,
    /// unseen definitions or paths for unused pub items — that detector is
    /// skipped for the affected scope, so findings stay trustworthy but the
    /// analysis is incomplete until the warnings are resolved.
    pub warnings: Vec<String>,
}

/// Analyze the workspace containing `path` and return all findings.
pub fn analyze(path: &Path) -> Result<Analysis> {
    let meta = metadata::load(path)?;
    let mut findings = Vec::new();
    let mut warnings = Vec::new();

    // Every target is a crate of its own for name resolution: a bin and the
    // lib it uses see different scopes, and the same file pulled into two
    // packages via `#[path]` is a separate module in each.
    let mut crates: Vec<resolve::CrateUnit> = Vec::new();
    // Package name to its library crate, so a dependency rename can be
    // attached to the crate the alias actually names.
    let mut lib_of_package: HashMap<&str, usize> = HashMap::new();

    for package in &meta.packages {
        let manifest_dir = package
            .manifest_path
            .parent()
            .context("manifest path has no parent directory")?;

        let warnings_before = warnings.len();
        let mut package_reachable: HashSet<PathBuf> = HashSet::new();
        for target in &package.targets {
            let files = modtree::resolve(&target.src_path, &mut warnings);
            for file in &files {
                package_reachable.insert(file.path.clone());
            }
            let names = crate_names(package, target);
            if !names.is_empty() {
                lib_of_package.insert(package.name.as_str(), crates.len());
            }
            crates.push(resolve::CrateUnit { names, files });
        }

        // An unparsable file or unresolved `mod` means the reachable set is
        // incomplete, and files it would have reached would be reported as
        // false-positive dead files — skip the check for this package.
        if warnings.len() > warnings_before {
            warnings.push(format!(
                "dead-file check skipped for package `{}`: module resolution was incomplete (see warnings above)",
                package.name
            ));
            continue;
        }

        // Dead-file detection only covers src/: tests/, examples/, and
        // benches/ roots are auto-discovered targets and already covered
        // above, but stray helper files there are usually intentional.
        let src_dir = manifest_dir.join("src");
        if src_dir.is_dir() {
            for file in modtree::rs_files_under(&src_dir) {
                if !package_reachable.contains(&file) {
                    findings.push(Finding {
                        kind: FindingKind::DeadFile,
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
            unused::find_unused_items(&crates, &mut warnings)
                .into_iter()
                .map(|item| Finding {
                    kind: if item.reexport {
                        FindingKind::UnusedReexport
                    } else {
                        FindingKind::UnusedPubItem
                    },
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

    findings.sort_by(|a, b| {
        (a.file.as_path(), a.line.unwrap_or(0)).cmp(&(b.file.as_path(), b.line.unwrap_or(0)))
    });

    Ok(Analysis {
        workspace_root: meta.workspace_root,
        findings,
        warnings,
    })
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
