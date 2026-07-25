//! Workspace discovery via `cargo metadata`.
//!
//! We shell out to `cargo metadata --no-deps` rather than reimplementing
//! manifest parsing: it is the canonical source of truth for workspace
//! membership, target roots (lib/bin/test/example/bench/build), and the
//! workspace root, and it works offline when no dependency resolution is
//! needed.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The subset of `cargo metadata` output that Deadwood needs.
#[derive(Debug, Deserialize)]
pub struct Metadata {
    /// With `--no-deps` this contains only workspace members.
    pub packages: Vec<Package>,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct Package {
    pub name: String,
    pub manifest_path: PathBuf,
    pub targets: Vec<Target>,
    /// Declared dependencies. Needed for the `rename` field: a dependency
    /// renamed in `Cargo.toml` is spelled by its alias in code, which is not
    /// derivable from the dependency's own package or lib name.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Deserialize)]
pub struct Dependency {
    /// The dependency's package name.
    pub name: String,
    /// The name code refers to it by, when it differs from `name`.
    #[serde(default)]
    pub rename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    /// Absolute path to the target's crate root file.
    pub src_path: PathBuf,
}

/// Run `cargo metadata` for the package or workspace at `path`.
///
/// `path` may be a directory containing a `Cargo.toml` or a manifest file
/// itself.
pub fn load(path: &Path) -> Result<Metadata> {
    let manifest = if path.is_dir() {
        path.join("Cargo.toml")
    } else {
        path.to_path_buf()
    };
    if !manifest.is_file() {
        bail!("no Cargo.toml found at `{}`", manifest.display());
    }

    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .context("failed to run `cargo metadata` (is cargo on PATH?)")?;

    if !output.status.success() {
        bail!(
            "`cargo metadata` failed for `{}`:\n{}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    serde_json::from_slice(&output.stdout).context("failed to parse `cargo metadata` output")
}
