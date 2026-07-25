//! The `deadwood.toml` configuration file.
//!
//! Deadwood runs with no configuration at all, and that has to keep working
//! byte for byte: an absent config file is exactly today's behavior, and
//! [`Config::default`] is the value that expresses it. Everything here is a
//! way to make Deadwood report *less*, never more.
//!
//! # Discovery
//!
//! Starting at the analyzed path, we walk up to the workspace root looking for
//! `deadwood.toml` and take the first one found; `--config PATH` overrides the
//! search and is an error if the file is missing. Relative patterns in the
//! file are resolved against the directory the file lives in, so a config
//! moves with the tree it describes.
//!
//! # Errors are loud
//!
//! Every parse problem is a hard failure (exit 2), and unknown keys are parse
//! problems: the structs are `#[serde(deny_unknown_fields)]` and the severity
//! table is keyed by [`crate::FindingKind`] itself, so a typo'd `ignor` or
//! `dead_fil` names the file, the key, and what was expected instead of
//! quietly doing nothing. A setting that silently has no effect is worse than
//! no setting, because the user believes it worked.
//!
//! # The settings, and what each is for
//!
//! - **`ignore`** — glob patterns for files that no finding may be reported
//!   about. Note the phrasing: an ignored file is still *read*. Its `use`
//!   declarations and paths still count as references, because generated code
//!   that calls your `pub fn` is still calling it, and forgetting that would
//!   turn every `ignore` entry into a source of false positives elsewhere.
//!   Ignoring a file suppresses findings about it; it does not make Deadwood
//!   pretend not to know what is in it. (One exception, in
//!   [`crate::modtree`]: a `mod` declaration pointing at a *missing* file that
//!   an ignore pattern covers is skipped silently rather than warned about,
//!   since there is nothing there to read.)
//! - **`severity`** — `deny`, `warn`, or `off` per finding kind. Only `deny`
//!   findings fail the run; `warn` prints and exits 0; `off` is never
//!   reported. The default is `deny` for every kind.
//! - **`public-api`** — the biggest real-world noise lever. A `pub` item in a
//!   library with consumers outside the workspace looks exactly like a dead
//!   one, and Deadwood cannot tell them apart, so it reports them as advisory
//!   findings. This is how a project says "this crate's surface is the API"
//!   once, instead of per item.
//! - **`dependencies`** — manifest entries the unused-dependency check must
//!   never judge. Some entries are load bearing without being named by any
//!   code: `getrandom = { features = ["js"] }` exists to turn on a feature of
//!   a *transitive* dependency, and `openssl = { features = ["vendored"] }` to
//!   select a native library. There is no syntactic signal that separates
//!   those from a stale entry, so the answer is user intent.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::FindingKind;
use crate::glob::Glob;

/// The file name Deadwood looks for when no `--config` is given.
pub const FILE_NAME: &str = "deadwood.toml";

/// How much a finding of some kind matters.
///
/// The exit code follows this and nothing else: a run fails only when it
/// produced at least one [`Severity::Deny`] finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Reported, and fails the run (exit 1). The default for every kind, which
    /// is what makes an absent config file a no-op.
    #[default]
    Deny,
    /// Reported, but the run still succeeds (exit 0).
    Warn,
    /// Never reported at all.
    Off,
}

impl Severity {
    /// How the severity is spelled in reports, matching the config file.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Deny => "deny",
            Severity::Warn => "warn",
            Severity::Off => "off",
        }
    }
}

/// A validated `deadwood.toml`, with its patterns compiled.
///
/// [`Config::default`] is the no-config-file behavior and is what every code
/// path falls back to.
#[derive(Debug, Default)]
pub struct Config {
    /// Directory relative patterns are resolved against: the one holding the
    /// config file. Empty when there is no config file, where nothing can
    /// match anyway.
    base: PathBuf,
    ignore: Vec<Glob>,
    severity: HashMap<FindingKind, Severity>,
    public_api: PublicApi,
    dependencies: DependencyAllowList,
}

impl Config {
    /// Read and validate the config file at `path`.
    ///
    /// A missing file is an error here: `--config` names a file the user
    /// expects to be used, and silently running without it would apply the
    /// wrong settings to a whole CI run.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read config file `{}`", path.display()))?;
        let raw: RawConfig = toml::from_str(&text)
            .with_context(|| format!("invalid config file `{}`", path.display()))?;
        Ok(Config {
            base: base_of(path),
            ignore: raw.ignore.iter().map(|p| Glob::path(p)).collect(),
            severity: raw.severity,
            public_api: PublicApi::compile(raw.public_api),
            dependencies: DependencyAllowList::compile(raw.dependencies),
        })
    }

    /// Find the config governing `analyzed`, walking up to `workspace_root`.
    ///
    /// The nearest file wins, so a package in a large workspace can carry its
    /// own. Finding none is not an error — it is the default configuration.
    pub fn discover(analyzed: &Path, workspace_root: &Path) -> Result<Config> {
        let start = if analyzed.is_dir() {
            analyzed.to_path_buf()
        } else {
            analyzed.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        // `cargo metadata` reports an absolute, symlink-resolved root, and the
        // analyzed path is usually relative; without the same treatment the
        // ancestor walk below would never recognize the root and would climb
        // out of the workspace.
        let start = start.canonicalize().unwrap_or(start);
        let root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        let mut current = if start.starts_with(&root) {
            Some(start.as_path())
        } else {
            // Nothing sensible to walk: fall back to the root alone.
            Some(root.as_path())
        };
        while let Some(directory) = current {
            let candidate = directory.join(FILE_NAME);
            if candidate.is_file() {
                return Config::load(&candidate);
            }
            if directory == root {
                break;
            }
            current = directory.parent();
        }
        Ok(Config::default())
    }

    /// The severity configured for `kind`, defaulting to [`Severity::Deny`].
    pub fn severity(&self, kind: FindingKind) -> Severity {
        self.severity.get(&kind).copied().unwrap_or_default()
    }

    /// The compiled `ignore` patterns, for the detectors that need to consult
    /// them before a finding exists.
    pub fn ignore(&self) -> Ignore<'_> {
        Ignore {
            base: &self.base,
            globs: &self.ignore,
        }
    }

    pub fn public_api(&self) -> &PublicApi {
        &self.public_api
    }

    pub fn dependencies(&self) -> &DependencyAllowList {
        &self.dependencies
    }
}

/// The `ignore` patterns, borrowed so they can be handed to module resolution
/// without dragging the whole config along.
#[derive(Clone, Copy)]
pub struct Ignore<'a> {
    base: &'a Path,
    globs: &'a [Glob],
}

impl Ignore<'_> {
    /// Whether no finding may be reported about `path`.
    ///
    /// A pattern that matches a directory covers everything under it, so
    /// `ignore = ["vendor"]` and `ignore = ["vendor/**"]` both silence the
    /// whole subtree — the distinction is a papercut nobody wants to debug.
    pub fn matches(&self, path: &Path) -> bool {
        if self.globs.is_empty() {
            return false;
        }
        let Ok(relative) = path.strip_prefix(self.base) else {
            // Outside the tree the config describes; its patterns say nothing
            // about it.
            return false;
        };
        let mut prefix = String::new();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                continue;
            };
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(&name.to_string_lossy());
            if self.globs.iter().any(|glob| glob.matches(&prefix)) {
                return true;
            }
        }
        false
    }
}

/// Crates and item paths whose `pub` surface is intentional API.
///
/// Deadwood can only see consumers inside the workspace, so a library's
/// exported items are indistinguishable from dead ones. Listing them here is
/// the project asserting what Deadwood cannot observe.
#[derive(Debug, Default)]
pub struct PublicApi {
    crates: HashSet<String>,
    items: Vec<Glob>,
}

impl PublicApi {
    fn compile(raw: RawPublicApi) -> PublicApi {
        PublicApi {
            crates: raw.crates.iter().map(|name| normalize(name)).collect(),
            items: raw
                .items
                .iter()
                .map(|pattern| Glob::item(&normalize(pattern)))
                .collect(),
        }
    }

    /// Whether an item is declared API rather than dead code.
    ///
    /// `krate` is the library crate the item belongs to, and is `None` for
    /// targets nothing outside the workspace can name at all (bins, tests,
    /// examples, benches). A `crates` listing never covers those — there are
    /// no external consumers to declare — and their `item_path` carries no
    /// crate segment for the same reason, so a listing written the documented
    /// way (`my-crate::module::Item`) cannot match one either.
    pub fn covers(&self, krate: Option<&str>, item_path: &str) -> bool {
        if let Some(krate) = krate
            && self.crates.contains(&normalize(krate))
        {
            return true;
        }
        let item_path = normalize(item_path);
        self.items.iter().any(|glob| glob.matches(&item_path))
    }
}

/// The directory a config file's relative patterns are resolved against.
///
/// It has to be absolute: the paths patterns are matched against come from
/// `cargo metadata` and are absolute and symlink-resolved, and `--config
/// deadwood.toml` on its own has no directory part at all — taking that
/// literally would leave every pattern matching nothing, silently.
fn base_of(config_file: &Path) -> PathBuf {
    let directory = match config_file.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    directory.canonicalize().unwrap_or(directory)
}

/// Cargo normalizes `-` to `_` in crate names, so `engine-core` and
/// `engine_core` name the same thing and a config file may spell either.
fn normalize(name: &str) -> String {
    name.replace('-', "_")
}

/// Manifest entries the unused-dependency check must never judge.
///
/// Matching is on the manifest key, exactly as written, because that is the
/// line the user would delete: a renamed entry
/// (`motor = { package = "engine-core" }`) is listed as `motor`. No globs —
/// a dependency list is short, explicit, and worth being explicit about.
#[derive(Debug, Default)]
pub struct DependencyAllowList {
    workspace: HashSet<String>,
    per_package: HashMap<String, HashSet<String>>,
}

impl DependencyAllowList {
    fn compile(raw: RawDependencies) -> DependencyAllowList {
        DependencyAllowList {
            workspace: raw.allow.into_iter().collect(),
            per_package: raw
                .allow_in
                .into_iter()
                .map(|(package, entries)| (package, entries.into_iter().collect()))
                .collect(),
        }
    }

    /// Whether `entry` in `package`'s manifest is exempt from the check.
    ///
    /// This means "do not judge this entry", not "this entry is unused": an
    /// allowlisted entry that code *does* reference is not an error and is
    /// not reported either way.
    pub fn allows(&self, package: &str, entry: &str) -> bool {
        self.workspace.contains(entry)
            || self
                .per_package
                .get(package)
                .is_some_and(|entries| entries.contains(entry))
    }
}

// -- the file format itself ------------------------------------------------
//
// Kept separate from the compiled types above so the wire format is readable
// in one place, and so `deny_unknown_fields` covers every table.

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawConfig {
    #[serde(default)]
    ignore: Vec<String>,
    /// Keyed by [`FindingKind`] itself, so a new finding kind is configurable
    /// the day it is added, with no plumbing here and no way for the two
    /// spellings to drift apart.
    #[serde(default)]
    severity: HashMap<FindingKind, Severity>,
    #[serde(default)]
    public_api: RawPublicApi,
    #[serde(default)]
    dependencies: RawDependencies,
    //
    // The baseline file from <https://github.com/rlorenzo/deadwood/issues/6>
    // belongs here, as `baseline: Option<PathBuf>` resolved against `base`.
    // It is deliberately not accepted yet: a key Deadwood parses and ignores
    // is the silent misconfiguration this module exists to prevent.
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawPublicApi {
    /// Whole crates whose `pub` surface is API.
    #[serde(default)]
    crates: Vec<String>,
    /// Glob patterns over `crate::module::Item` paths.
    #[serde(default)]
    items: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct RawDependencies {
    /// Manifest keys exempt in every package of the workspace.
    #[serde(default)]
    allow: Vec<String>,
    /// Manifest keys exempt in one named package.
    #[serde(default)]
    allow_in: HashMap<String, Vec<String>>,
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Write `contents` to `deadwood.toml` in a fresh directory and load it.
    fn load(contents: &str) -> Result<Config> {
        let dir = std::env::temp_dir().join(format!(
            "deadwood-config-test-{}-{:p}",
            std::process::id(),
            contents
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        fs::write(&path, contents).unwrap();
        Config::load(&path)
    }

    /// The single most important property: no config file is today's behavior.
    #[test]
    fn the_default_config_changes_nothing() {
        let config = Config::default();
        for kind in [
            FindingKind::DeadFile,
            FindingKind::UnusedPubItem,
            FindingKind::UnusedReexport,
            FindingKind::UnusedDependency,
        ] {
            assert_eq!(config.severity(kind), Severity::Deny);
        }
        assert!(!config.ignore().matches(Path::new("/ws/src/lib.rs")));
        assert!(!config.public_api().covers(Some("anything"), "any::path"));
        assert!(!config.dependencies().allows("pkg", "entry"));
    }

    #[test]
    fn severity_is_keyed_by_the_finding_kinds_own_serde_tag() {
        let config =
            load("[severity]\ndead_file = \"warn\"\nunused_dependency = \"off\"\n").unwrap();
        assert_eq!(config.severity(FindingKind::DeadFile), Severity::Warn);
        assert_eq!(
            config.severity(FindingKind::UnusedDependency),
            Severity::Off
        );
        // Anything not mentioned keeps the default.
        assert_eq!(config.severity(FindingKind::UnusedPubItem), Severity::Deny);
    }

    /// A typo that silently does nothing is the failure mode this whole module
    /// is shaped to avoid, so every misspelling must name the file, the key,
    /// and what was expected.
    #[test]
    fn an_unknown_key_is_an_error_naming_the_file_and_the_alternatives() {
        let err = format!("{:#}", load("ignor = [\"a\"]\n").unwrap_err());
        assert!(err.contains("invalid config file"), "{err}");
        assert!(err.contains(FILE_NAME), "{err}");
        assert!(err.contains("unknown field `ignor`"), "{err}");
        assert!(
            err.contains("`ignore`"),
            "expected keys must be listed: {err}"
        );
    }

    #[test]
    fn an_unknown_finding_kind_or_severity_is_an_error() {
        let kind = format!(
            "{:#}",
            load("[severity]\ndead_fil = \"warn\"\n").unwrap_err()
        );
        assert!(kind.contains("unknown variant `dead_fil`"), "{kind}");
        assert!(kind.contains("dead_file"), "{kind}");

        let level = format!(
            "{:#}",
            load("[severity]\ndead_file = \"loud\"\n").unwrap_err()
        );
        assert!(level.contains("unknown variant `loud`"), "{level}");
        assert!(level.contains("deny"), "{level}");
    }

    #[test]
    fn a_missing_config_file_is_an_error_rather_than_a_silent_default() {
        let err = format!(
            "{:#}",
            Config::load(Path::new("/nonexistent/deadwood.toml")).unwrap_err()
        );
        assert!(err.contains("could not read config file"), "{err}");
    }

    #[test]
    fn ignore_patterns_are_relative_to_the_config_file() {
        let config = load("ignore = [\"src/generated/**\", \"vendor\"]\n").unwrap();
        let base = config.base.clone();
        let ignore = config.ignore();
        assert!(ignore.matches(&base.join("src/generated/a.rs")));
        assert!(ignore.matches(&base.join("src/generated")));
        // A pattern naming a directory covers the whole subtree under it.
        assert!(ignore.matches(&base.join("vendor/deep/a.rs")));
        assert!(!ignore.matches(&base.join("src/lib.rs")));
        // A path outside the configured tree is not matched at all.
        assert!(!ignore.matches(Path::new("/elsewhere/vendor/a.rs")));
    }

    #[test]
    fn public_api_covers_whole_crates_and_globbed_item_paths() {
        let config =
            load("[public-api]\ncrates = [\"engine-core\"]\nitems = [\"app::prelude::*\"]\n")
                .unwrap();
        let api = config.public_api();
        // Cargo's dash/underscore equivalence holds in both directions.
        assert!(api.covers(Some("engine_core"), "engine_core::api::Thing"));
        assert!(api.covers(Some("app"), "app::prelude::Thing"));
        assert!(!api.covers(Some("app"), "app::internals::Thing"));
        // A target nothing outside the workspace can name is never covered by
        // a crate listing, because it has no external consumers.
        assert!(!api.covers(None, "some::path::Thing"));
    }

    #[test]
    fn dependency_allowlists_apply_workspace_wide_and_per_package() {
        let config = load(
            "[dependencies]\nallow = [\"getrandom\"]\n\n[dependencies.allow-in]\napp = [\"openssl\"]\n",
        )
        .unwrap();
        let allow = config.dependencies();
        assert!(allow.allows("anything", "getrandom"));
        assert!(allow.allows("app", "openssl"));
        assert!(!allow.allows("other", "openssl"));
    }
}
