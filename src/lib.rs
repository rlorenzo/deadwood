//! Deadwood: a codebase health analyzer for Rust workspaces.
//!
//! The library entry point is [`analyze`], which discovers a workspace via
//! `cargo metadata`, resolves each package's module tree, and runs the
//! detectors that are currently implemented:
//!
//! - **Dead module files**: `.rs` files under a package's `src/` that are not
//!   reachable from any target root through `mod` declarations.
//! - **Unused public items**: fully-`pub` items whose name is never mentioned
//!   anywhere else in the workspace. This is a name-based heuristic (see
//!   [`unused`] for the exact rules and known false-negative bias).

pub mod metadata;
pub mod modtree;
pub mod report;
pub mod unused;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// The category of a finding, used for grouping and JSON output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A source file not reachable from any crate root via `mod` declarations.
    DeadFile,
    /// A `pub` item never referenced by name anywhere else in the workspace.
    UnusedPubItem,
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
    /// `mod` declarations). These make results incomplete, not wrong.
    pub warnings: Vec<String>,
}

/// Analyze the workspace containing `path` and return all findings.
pub fn analyze(path: &Path) -> Result<Analysis> {
    let meta = metadata::load(path)?;
    let mut findings = Vec::new();
    let mut warnings = Vec::new();

    // Parse every reachable file once; both detectors share the results.
    // Dedup is workspace-wide, not per-package: a file shared by several
    // packages (e.g. via `#[path]`) must enter the identifier census exactly
    // once, or its definitions would count as uses and hide dead items.
    let mut reachable: Vec<modtree::ParsedFile> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for package in &meta.packages {
        let manifest_dir = package
            .manifest_path
            .parent()
            .context("manifest path has no parent directory")?;

        let mut package_reachable: HashSet<PathBuf> = HashSet::new();
        for target in &package.targets {
            let tree = modtree::resolve(&target.src_path, &mut warnings);
            for file in tree {
                package_reachable.insert(file.path.clone());
                if seen.insert(file.path.clone()) {
                    reachable.push(file);
                }
            }
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

    findings.extend(
        unused::find_unused_pub_items(&reachable, &mut warnings)
            .into_iter()
            .map(|item| Finding {
                kind: FindingKind::UnusedPubItem,
                file: relative_to(&item.file, &meta.workspace_root),
                line: Some(item.line),
                name: Some(item.name.clone()),
                message: format!(
                    "pub {} `{}` is never referenced by name anywhere in this workspace",
                    item.kind, item.name
                ),
            }),
    );

    findings.sort_by(|a, b| {
        (a.file.as_path(), a.line.unwrap_or(0)).cmp(&(b.file.as_path(), b.line.unwrap_or(0)))
    });

    Ok(Analysis {
        workspace_root: meta.workspace_root,
        findings,
        warnings,
    })
}

fn relative_to(path: &Path, root: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}
