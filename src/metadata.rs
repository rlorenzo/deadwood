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
    /// Lib and proc-macro target names that differ from their package's
    /// name, keyed by package name — `md-5` builds a lib named `md5`,
    /// `rustls-webpki` one named `webpki`, and code spells the *lib* name,
    /// so matching mentions against the package name reports such an entry
    /// unused against code that uses it
    /// ([#62](https://github.com/rlorenzo/deadwood-rs/issues/62)).
    ///
    /// Filled by a full `cargo metadata --frozen --all-features` (default
    /// features alone if that fails) when the workspace has a current lockfile and a cached dependency graph — true of any checkout
    /// that has built once — plus every workspace member's own targets, and
    /// empty otherwise, which leaves the package-name
    /// heuristic exactly as it was. Exact where possible, unchanged where
    /// not.
    #[serde(skip)]
    pub lib_names: HashMap<String, String>,
    /// Proc macros from outside the workspace that members use, read from the
    /// same full resolution as [`Metadata::lib_names`] and empty where it
    /// fails. See [`ExternalMacro`].
    #[serde(skip)]
    pub external_macros: Vec<ExternalMacro>,
}

impl Metadata {
    /// Every workspace member the members in `roots` depend on, however
    /// indirectly — the roots themselves only when a cycle leads back.
    pub fn workspace_reach<'a>(
        &'a self,
        roots: impl IntoIterator<Item = &'a str>,
    ) -> HashSet<&'a str> {
        let members: HashMap<&str, &Package> = self
            .packages
            .iter()
            .map(|package| (package.name.as_str(), package))
            .collect();
        let mut seen = HashSet::new();
        let mut pending: Vec<&str> = roots.into_iter().collect();
        while let Some(name) = pending.pop() {
            let Some(member) = members.get(name) else {
                continue;
            };
            for dependency in &member.dependencies {
                if let Some((&next, _)) = members.get_key_value(dependency.name.as_str())
                    && seen.insert(next)
                {
                    pending.push(next);
                }
            }
        }
        seen
    }

    /// The workspace member named `name`.
    pub fn member(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|package| package.name == name)
    }
}

/// A proc-macro package from outside the workspace, with what is needed to
/// read what it can expand to.
///
/// `tauri::generate_context!` expands to `::serde_json::Value`, so an app's
/// `serde_json` entry looks unused; the path is written by `tauri-utils`, a
/// library `tauri-macros` builds its output with. So the sources are the
/// proc-macro package and every package it depends on, however indirectly,
/// that itself depends on `quote` — the crates that write code — other than
/// `syn`, `quote` and `proc-macro2`, which write none of a macro's output.
#[derive(Debug)]
pub struct ExternalMacro {
    /// The packages to read, proc-macro package first.
    pub sources: Vec<Package>,
    /// The workspace members whose resolved dependencies reach it, however
    /// indirectly — a derive is as often re-exported by a library as depended
    /// on directly.
    pub users: HashSet<String>,
}

#[derive(Debug, Clone, Deserialize)]
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
    /// Whether one of the package's targets is a proc macro.
    pub fn is_proc_macro(&self) -> bool {
        self.targets.iter().any(Target::is_proc_macro)
    }

    /// Whether the package writes code for a macro: it depends on `quote`,
    /// and is not one of the crates every macro builds on, which write none
    /// of a macro's output themselves.
    pub fn writes_code(&self) -> bool {
        !["syn", "quote", "proc-macro2"].contains(&self.name.as_str())
            && self
                .dependencies
                .iter()
                .any(|dependency| dependency.name == "quote")
    }

    /// Dependency names the `[features]` table refers to, as
    /// [`Dependency::feature_key`] spells them. A feature writes the manifest
    /// key, which is not what code spells when the lib target has its own name.
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
        // The features Cargo synthesizes for an optional dependency no
        // `dep:` entry mentions: `foo = ["dep:foo"]`, under the entry's name.
        let implicit = |feature: &str| {
            self.features.get(feature).is_some_and(|enabled| {
                enabled.len() == 1 && enabled[0].strip_prefix("dep:") == Some(feature)
            })
        };
        let mut names = HashSet::new();
        for (feature, enabled) in &self.features {
            for entry in enabled {
                let name = match entry.split_once('/') {
                    Some((dependency, _)) => dependency.trim_end_matches('?'),
                    None => match entry.strip_prefix("dep:") {
                        Some(dependency) => dependency,
                        // A bare entry names another feature of this package,
                        // and only an implicit one is a dependency: ruff's
                        // `ruff_instrumented = ["criterion", ..]` enables the
                        // optional `criterion` through the feature Cargo made
                        // for it.
                        None if implicit(entry) => entry,
                        None => continue,
                    },
                };
                // The implicit feature itself is not a mention — every
                // optional entry has one — only another feature naming it is.
                if implicit(feature) && name == feature {
                    continue;
                }
                names.insert(name.replace('-', "_"));
            }
        }
        names
    }
}

#[derive(Debug, Clone, Deserialize)]
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
    /// The features the entry asks for (`features = ["..."]`). What a doubled
    /// declaration turns on beyond its `[dependencies]` copy is read from
    /// here, so a dev copy that exists only for its feature list is not
    /// mistaken for a stale duplicate.
    #[serde(default)]
    pub features: Vec<String>,
    /// Whether the entry keeps the dependency's default features on. `cargo
    /// metadata` reports the written value; the field defaults to `true`
    /// exactly as the manifest key does.
    #[serde(default = "default_features_default")]
    pub uses_default_features: bool,
}

/// The default of `default-features`: on unless the manifest says otherwise.
fn default_features_default() -> bool {
    true
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

    /// The manifest key in the form
    /// [`Package::dependencies_named_by_features`] reports it.
    pub fn feature_key(&self) -> String {
        self.manifest_name().replace('-', "_")
    }

    /// The identifier code spells this dependency with; cargo normalizes `-`
    /// to `_` in crate names.
    ///
    /// Three sources, in the order cargo consults them: a `rename` sets the
    /// extern name outright; otherwise the dependency's own lib target name
    /// is what code spells — `md-5` builds a lib named `md5`, and
    /// `lib_names` carries every such difference a frozen resolution or the
    /// member list could see ([`Metadata::lib_names`]); and with neither, the package
    /// name normalized, which is also every crate whose lib keeps the
    /// default name.
    pub fn crate_name(&self, lib_names: &HashMap<String, String>) -> String {
        if self.rename.is_none()
            && let Some(lib) = lib_names.get(&self.name)
        {
            return lib.clone();
        }
        self.feature_key()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: Vec<String>,
    /// Absolute path to the target's crate root file.
    pub src_path: PathBuf,
}

impl Target {
    /// Whether other crates can name the target: a library of any kind,
    /// proc macros included.
    pub fn is_lib(&self) -> bool {
        const LIB_KINDS: &[&str] = &["lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"];
        self.kind
            .iter()
            .any(|kind| LIB_KINDS.contains(&kind.as_str()))
    }

    pub fn is_proc_macro(&self) -> bool {
        self.kind.iter().any(|kind| kind == "proc-macro")
    }
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

    let mut metadata: Metadata = serde_json::from_slice(&output.stdout)
        .context("failed to parse `cargo metadata` output")?;
    let full = full_resolution(&manifest);
    metadata.lib_names = full.as_ref().map_or_else(HashMap::new, |full| {
        full.packages
            .iter()
            .filter_map(|package| renamed_lib(&package.package.name, &package.package.targets))
            .collect()
    });
    metadata.external_macros = full.map(external_macros).unwrap_or_default();
    // Workspace members are in front of us with or without a resolvable
    // graph, so a member's renamed lib never depends on the cache the full
    // resolution needs.
    for package in &metadata.packages {
        if let Some((name, lib_name)) = renamed_lib(&package.name, &package.targets) {
            metadata.lib_names.insert(name, lib_name);
        }
    }
    Ok(metadata)
}

/// The package's lib (or proc-macro) target name when it differs from the
/// package name — the key is the package name verbatim, dashes and all,
/// because that is how a `Dependency` names its package; only the *value* is
/// normalized to the underscore spelling code uses.
fn renamed_lib(package: &str, targets: &[Target]) -> Option<(String, String)> {
    let lib = targets.iter().find(|target| target.is_lib())?;
    let lib_name = lib.name.replace('-', "_");
    (lib_name != package.replace('-', "_")).then(|| (package.to_string(), lib_name))
}

/// A full, *frozen* resolution of the workspace, which answers
/// [`Metadata::lib_names`] and [`Metadata::external_macros`].
///
/// Failure of any kind — a missing or outdated lockfile, a cold registry
/// cache, an unresolvable fixture manifest — yields nothing, silently: the fallback is the
/// package-name heuristic that has always run, and a warning here would fire
/// on every fresh checkout for a gap that costs findings rather than
/// inventing them. Frozen implies offline, so the tenet holds: Deadwood
/// never reaches for the network, it only reads what some build already
/// fetched.
fn full_resolution(manifest: &Path) -> Option<Full> {
    // `--frozen` rather than `--offline` alone: a full resolution can write
    // or update the workspace's `Cargo.lock`, and a linter must not leave a
    // mark on the tree it reads. Frozen asserts the lockfile is present and
    // current and errs into the fallback otherwise — the missing-lock case
    // is a fresh project the heuristic already serves.
    first_resolution(|features| {
        let output = Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--frozen"])
            .args(features)
            .arg("--manifest-path")
            .arg(manifest)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        serde_json::from_slice::<Full>(&output.stdout).ok()
    })
}

/// A full `cargo metadata`: every package the resolution reaches, and the
/// graph between them.
#[derive(Deserialize)]
struct Full {
    packages: Vec<FullPackage>,
    workspace_members: Vec<String>,
    resolve: Option<Resolve>,
}

#[derive(Deserialize)]
struct FullPackage {
    id: String,
    #[serde(flatten)]
    package: Package,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

/// Every external proc-macro package some workspace member reaches.
fn external_macros(full: Full) -> Vec<ExternalMacro> {
    let Some(resolve) = full.resolve else {
        return Vec::new();
    };
    let edges: HashMap<&str, &[String]> = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.dependencies.as_slice()))
        .collect();
    let by_id: HashMap<&str, &Package> = full
        .packages
        .iter()
        .map(|package| (package.id.as_str(), &package.package))
        .collect();
    let members: HashSet<&str> = full.workspace_members.iter().map(String::as_str).collect();
    let reach = |from: &str| -> HashSet<&str> {
        let mut seen = HashSet::new();
        let mut pending = vec![from];
        while let Some(id) = pending.pop() {
            for next in edges.get(id).copied().unwrap_or_default() {
                if seen.insert(next.as_str()) {
                    pending.push(next.as_str());
                }
            }
        }
        seen
    };

    let mut users: HashMap<&str, HashSet<String>> = HashMap::new();
    for member in &members {
        let Some(member_package) = by_id.get(member) else {
            continue;
        };
        for id in reach(member) {
            if !members.contains(id) && by_id.get(id).is_some_and(|package| package.is_proc_macro())
            {
                users
                    .entry(id)
                    .or_default()
                    .insert(member_package.name.clone());
            }
        }
    }
    users
        .into_iter()
        .filter_map(|(id, users)| {
            let mut sources = vec![(*by_id.get(id)?).clone()];
            for helper in reach(id) {
                if let Some(package) = by_id.get(helper)
                    && !members.contains(helper)
                    && package.writes_code()
                {
                    sources.push((*package).clone());
                }
            }
            Some(ExternalMacro { sources, users })
        })
        .collect()
}

/// Resolve with every feature on, as the analysis itself does by default, so
/// a non-default optional dependency's lib name is seen. A checkout built only
/// with default features may not have those crates cached, and that must not
/// cost it the lib names it does have, so default features are the fallback.
fn first_resolution<T>(mut resolve: impl FnMut(&[&str]) -> Option<T>) -> Option<T> {
    resolve(&["--all-features"]).or_else(|| resolve(&[]))
}

#[cfg(test)]
mod tests {
    /// All features come first, and failing there still leaves default features.
    #[test]
    fn resolution_falls_back_from_all_features_to_default_features() {
        let mut tried = Vec::new();
        let answer = super::first_resolution(|features| {
            tried.push(features.join(" "));
            features.is_empty().then_some("default")
        });
        assert_eq!(answer, Some("default"));
        assert_eq!(tried, ["--all-features", ""]);

        let mut calls = 0;
        let answer = super::first_resolution(|_| {
            calls += 1;
            Some("all")
        });
        assert_eq!((answer, calls), (Some("all"), 1));
    }

    /// The manifest key `default-features` is on unless written: an entry
    /// that does not spell it keeps the dependency's default features, which
    /// is what the doubled-copy check reads off this field.
    #[test]
    fn an_unwritten_default_features_key_is_on() {
        let dependency: super::Dependency =
            serde_json::from_value(serde_json::json!({ "name": "plain" })).unwrap();
        assert!(dependency.uses_default_features);
    }

    fn package(name: &str, kind: &str, dependencies: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "id": name,
            "name": name,
            "manifest_path": format!("/ws/{name}/Cargo.toml"),
            "targets": [{ "name": name, "kind": [kind], "src_path": format!("/ws/{name}/src/lib.rs") }],
            "dependencies": dependencies.iter().map(|name| serde_json::json!({ "name": name })).collect::<Vec<_>>(),
        })
    }

    fn parse(value: serde_json::Value) -> super::Package {
        serde_json::from_value(value).unwrap()
    }

    /// A package is a proc macro by one of its targets, not by default.
    #[test]
    fn a_package_is_a_proc_macro_only_by_a_proc_macro_target() {
        assert!(parse(package("derive", "proc-macro", &[])).is_proc_macro());
        assert!(!parse(package("plain", "lib", &[])).is_proc_macro());
    }

    /// Writing code takes both halves: a `quote` dependency, and not being
    /// one of the crates every macro builds on.
    #[test]
    fn writing_code_takes_quote_and_not_being_a_macro_building_block() {
        assert!(parse(package("helper", "lib", &["quote"])).writes_code());
        assert!(!parse(package("helper", "lib", &["serde"])).writes_code());
        assert!(!parse(package("syn", "lib", &["quote"])).writes_code());
    }

    /// Only proc macros from outside the workspace count: a member proc macro
    /// is read as workspace code, and an external plain library is not a
    /// macro. The macro's helper that writes code is read with it.
    #[test]
    fn external_macros_are_the_non_member_proc_macros_members_reach() {
        let node = |id: &str, deps: &[&str]| serde_json::json!({ "id": id, "dependencies": deps });
        let full: super::Full = serde_json::from_value(serde_json::json!({
            "packages": [
                package("app", "lib", &[]),
                package("ws_macro", "proc-macro", &[]),
                package("ext_derive", "proc-macro", &[]),
                package("ext_helper", "lib", &["quote"]),
                package("ext_lib", "lib", &[]),
                package("quote", "lib", &[]),
            ],
            "workspace_members": ["app", "ws_macro"],
            "resolve": { "nodes": [
                node("app", &["ws_macro", "ext_derive", "ext_lib"]),
                node("ext_derive", &["ext_helper"]),
                node("ext_helper", &["quote"]),
            ]},
        }))
        .unwrap();
        let macros = super::external_macros(full);
        assert_eq!(macros.len(), 1);
        let sources: Vec<&str> = macros[0].sources.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(sources, ["ext_derive", "ext_helper"]);
        assert_eq!(macros[0].users, ["app".to_string()].into());
    }
}
