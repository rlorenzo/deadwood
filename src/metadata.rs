//! Workspace discovery via `cargo metadata`.
//!
//! We shell out to `cargo metadata --no-deps` rather than reimplementing
//! manifest parsing: it is the canonical source of truth for workspace
//! membership, target roots (lib/bin/test/example/bench/build), and the
//! workspace root, and it works offline when no dependency resolution is
//! needed.

use std::collections::{HashMap, HashSet};
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
    /// The `[features]` table: feature name to the list of features it
    /// enables. Entries can name dependencies (`dep:foo`, `foo/bar`), which
    /// is a use of the dependency that no code shows.
    #[serde(default)]
    pub features: HashMap<String, Vec<String>>,
}

impl Package {
    /// Dependency names the `[features]` table refers to, as code would spell
    /// them.
    ///
    /// A feature can enable an optional dependency (`dep:foo`) or turn on a
    /// feature of one (`foo/bar`, `foo?/bar`). Either way the entry is load
    /// bearing: deleting the dependency would break the feature.
    ///
    /// With one exception. Cargo synthesizes `foo = ["dep:foo"]` for every
    /// optional dependency no other feature mentions, and reports it here
    /// exactly like a hand-written feature. That entry is the dependency
    /// restated, not a second place naming it — counting it would keep every
    /// optional dependency alive by its own existence, which is precisely the
    /// verdict the `cfg` matrix now makes it possible to reach.
    pub fn dependencies_named_by_features(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for (feature, enabled) in &self.features {
            for entry in enabled {
                // A bare entry names another feature of this package, not a
                // dependency, and must not be mistaken for one.
                let name = match entry.split_once('/') {
                    Some((dependency, _)) => dependency.trim_end_matches('?'),
                    None => match entry.strip_prefix("dep:") {
                        Some(dependency) => dependency,
                        None => continue,
                    },
                };
                if enabled.len() == 1 && name == feature && entry.starts_with("dep:") {
                    continue;
                }
                names.insert(name.replace('-', "_"));
            }
        }
        names
    }
}

#[derive(Debug, Deserialize)]
pub struct Dependency {
    /// The dependency's package name.
    pub name: String,
    /// The name code refers to it by, when it differs from `name`.
    #[serde(default)]
    pub rename: Option<String>,
    /// `null` for `[dependencies]`, `"dev"` for `[dev-dependencies]`,
    /// `"build"` for `[build-dependencies]`. Which targets may refer to the
    /// dependency at all depends on this.
    #[serde(default)]
    pub kind: Option<String>,
    /// Whether the entry is `optional = true`, i.e. pulled in by a feature.
    #[serde(default)]
    pub optional: bool,
    /// The `cfg(...)` or target triple of a
    /// `[target.'...'.dependencies]` table, if the entry came from one.
    #[serde(default)]
    pub target: Option<String>,
}

/// Which dependency table an entry came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Normal,
    Development,
    Build,
}

impl Dependency {
    /// The table this entry was declared in.
    ///
    /// `cargo metadata` spells the kind as `null`/`"dev"`/`"build"`; an
    /// unrecognized value is treated as a normal dependency, which is the
    /// most permissive reading (it is satisfied by the widest set of
    /// targets).
    pub fn dependency_kind(&self) -> DependencyKind {
        match self.kind.as_deref() {
            Some("dev") => DependencyKind::Development,
            Some("build") => DependencyKind::Build,
            _ => DependencyKind::Normal,
        }
    }

    /// The entry as it is written in `Cargo.toml`: the rename when there is
    /// one, since that is the key the user would have to delete.
    pub fn manifest_name(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.name)
    }

    /// The identifier code spells this dependency with; cargo normalizes `-`
    /// to `_` in crate names.
    pub fn crate_name(&self) -> String {
        self.manifest_name().replace('-', "_")
    }
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
