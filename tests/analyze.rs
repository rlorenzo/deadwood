//! End-to-end tests: run the full analysis on the fixture packages under
//! `tests/fixtures/` and check the findings. Requires `cargo` on PATH (used
//! for `cargo metadata`), which is a given anywhere `cargo test` runs.

use std::path::{Path, PathBuf};

use deadwood::config::Severity;
use deadwood::{Analysis, FindingKind, analyze};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Analyze the named fixture, asserting the run was complete: any warning
/// makes a detector skip, which would make the assertions below vacuous.
fn analyze_fixture(name: &str) -> Analysis {
    let analysis =
        analyze(&fixtures().join(name), None).expect("analysis should succeed on the fixture");
    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );
    analysis
}

/// Analyze a fixture under one of the config files stored beside it.
fn analyze_configured(name: &str, config: &str) -> Analysis {
    let fixture = fixtures().join(name);
    let analysis = analyze(&fixture, Some(&fixture.join(config)))
        .unwrap_or_else(|err| panic!("`{config}` should load: {err:#}"));
    assert!(
        analysis.warnings.is_empty(),
        "unexpected warnings: {:?}",
        analysis.warnings
    );
    analysis
}

/// Assert the finding named `name` says `expected`.
///
/// The two unused-pub messages are two different claims — "nothing names it"
/// and "only unreachable things do" — so which one is made is part of what a
/// test can pin.
fn assert_finding_message(analysis: &Analysis, name: &str, expected: &str) {
    let finding = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("`{name}` should be reported: {:?}", analysis.findings));
    assert!(
        finding.message.contains(expected),
        "`{name}` should be reported as `{expected}`, got `{}`",
        finding.message
    );
}

/// Findings of one kind, as `(file, name)` pairs in report order.
fn reported(analysis: &Analysis, kind: FindingKind) -> Vec<(String, &str)> {
    analysis
        .findings
        .iter()
        .filter(|f| f.kind == kind)
        .map(|f| {
            (
                f.file.display().to_string(),
                f.name.as_deref().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn detects_dead_file_and_unused_pub_item() {
    let analysis = analyze_fixture("simple");

    let dead_files: Vec<&PathBuf> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::DeadFile)
        .map(|f| &f.file)
        .collect();
    assert_eq!(dead_files, vec![&PathBuf::from("src/orphan.rs")]);

    let unused: Vec<&str> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::UnusedPubItem)
        .filter_map(|f| f.name.as_deref())
        .collect();
    // `entry` and `dead_fn` are pub and referenced nowhere else in the
    // fixture workspace; `helper` is pub but called from lib.rs, and
    // `lonely` lives in a dead file, which is reported as a whole instead.
    assert_eq!(unused, vec!["entry", "dead_fn"]);

    // Findings carry 1-based line numbers pointing at the item name.
    let dead_fn = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some("dead_fn"))
        .unwrap();
    assert_eq!(dead_fn.line, Some(7));
    assert_eq!(dead_fn.file, PathBuf::from("src/lib.rs"));
}

/// A module loaded via `#[path]` keeps the stem-based directory rule for its
/// own children: `#[path = "renamed_file.rs"] mod alias;` with `mod child;`
/// inside must find `src/renamed_file/child.rs`, not `src/child.rs`.
#[test]
fn path_attr_module_children_resolve_in_stem_directory() {
    let analysis = analyze_fixture("pathmod");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::DeadFile),
        "no file in the fixture is dead: {:?}",
        analysis.findings
    );
    // `seven` is called through the aliased module, so it must not be flagged.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("seven")),
        "seven is referenced and must not be reported"
    );
}

/// When a module file cannot be parsed, both detectors must skip instead of
/// reporting findings from incomplete data: the broken file could declare
/// `mod`s (so no dead-file findings) and could use any item (so no
/// unused-pub findings).
#[test]
fn detectors_skip_when_module_resolution_is_incomplete() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/broken");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert!(
        analysis.findings.is_empty(),
        "incomplete data must produce no findings: {:?}",
        analysis.findings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("could not parse")),
        "the parse failure must be surfaced: {:?}",
        analysis.warnings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("dead-file check skipped")),
        "the dead-file skip must be surfaced: {:?}",
        analysis.warnings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("unused-pub and test-only checks skipped")),
        "the skip must be surfaced, and must name both kinds the one resolution \
         pass produces: {:?}",
        analysis.warnings
    );
    // A file that failed to parse could hold the only reference to a
    // dependency, and could hold it in code from a target no other file
    // covers, so both dependency checks skip the package too.
    for check in ["unused-dependency", "misplaced-dependency"] {
        assert!(
            analysis
                .warnings
                .iter()
                .any(|w| w.contains(&format!("{check} check skipped"))),
            "the {check} skip must be surfaced: {:?}",
            analysis.warnings
        );
    }
}

/// A file included by several workspace members via `#[path]` is a module of
/// each crate, so its dead pub items are still reported — once, not per
/// including package.
#[test]
fn file_shared_between_packages_is_counted_once() {
    let analysis = analyze_fixture("shared");
    let shared_dead_reports = analysis
        .findings
        .iter()
        .filter(|f| f.name.as_deref() == Some("shared_dead"))
        .count();
    assert_eq!(
        shared_dead_reports, 1,
        "shared_dead is unused and must be reported exactly once: {:?}",
        analysis.findings
    );

    // The same file is compiled by two packages with different feature
    // tables, so `#[cfg(feature = "wide")]` in it is impossible for the member
    // that does not declare `wide` and ordinary for the one that does. A gate
    // is only dead by construction when *no* package that compiles it can
    // satisfy it — otherwise every shared file would report against whichever
    // member happened to be judged first.
    assert!(
        reported(&analysis, FindingKind::UnsatisfiableCfg).is_empty(),
        "one member declares the feature, so the gate can hold: {:?}",
        analysis.findings
    );
}

/// The three classes of dead code the old identifier census could not see:
/// an item sharing a name with a used one, a type mentioned only by its own
/// `impl` block, and a `pub use` re-export nothing goes through. Renamed and
/// nested imports and `crate::`/`self::`/`super::` paths all resolve, so the
/// items they reach stay unreported.
#[test]
fn resolution_sees_through_names_impls_and_reexports() {
    let analysis = analyze_fixture("paths");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // Only mentioned by `impl ImplOnly`, including inside its body.
            ("src/alpha.rs".to_string(), "ImplOnly"),
            // `alpha::collision` is the one every path resolves to; a census
            // of the name "collision" hid this one behind it.
            ("src/beta.rs".to_string(), "collision"),
            // Only mentioned by `impl crate::qualified::Selfish`, whose body
            // spells the same type bare.
            ("src/qualified.rs".to_string(), "Selfish"),
            // Same, one module down; the bare `Wrapper` at that impl is a
            // different type, so it is not mistaken for a self-reference.
            ("src/qualified.rs".to_string(), "Wrapper"),
            // Reached only from inside `impl ...::inner::Wrapper`, which is an
            // impl of the dead type above it.
            ("src/qualified.rs".to_string(), "Other"),
            // Reached only through the dead re-exports below.
            ("src/surface/hidden.rs".to_string(), "Ignored"),
            ("src/surface/hidden.rs".to_string(), "Renamed"),
        ]
    );

    // The bare `Wrapper` in that impl body is `inner::Other`, renamed by a
    // `use`. `Other` is reported because the impl holding that path is itself
    // unreachable — not because the path was dropped, which is what treating
    // the bare name as the impl's self-reference would have done. The two read
    // apart in the message, so the assertion can tell them apart too.
    assert_finding_message(
        &analysis,
        "Other",
        "is referenced only from items that nothing reaches",
    );

    assert_eq!(
        reported(&analysis, FindingKind::UnusedReexport),
        vec![
            ("src/surface.rs".to_string(), "Ignored"),
            ("src/surface.rs".to_string(), "Alias"),
        ]
    );

    // `Exposed` is reached *through* its re-export, so neither is reported:
    // a `use` names its target on the bound name's behalf, and that name is
    // live. The other two definitions come out beside their re-exports
    // because a dead re-export and the definition under it are two deletions.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("Exposed")),
        "a live re-export keeps its target alive: {:?}",
        analysis.findings
    );

    // `surface` is private, so its dead re-exports are dead with certainty.
    // `facade` is `pub` from the crate root of a library, where an unused
    // re-export is the public-API idiom rather than a finding.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file.starts_with("src/facade")),
        "a re-export on the public surface must be left alone: {:?}",
        analysis.findings
    );
}

/// Anything Deadwood cannot resolve counts as a use: identifiers in macro
/// input, and names in a module whose scope has a glob import that does not
/// lead into the workspace. A glob that *does* is expanded instead of giving
/// up, so it hides nothing.
#[test]
fn unresolvable_paths_keep_their_targets_alive() {
    let analysis = analyze_fixture("macros");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![("src/glob_source.rs".to_string(), "never_named")],
        "only the item behind a followed glob that nobody names is dead"
    );
}

/// A dead subsystem comes out in one run, not one layer per run: `orphan` is
/// named by nothing, so `helper` — which only `orphan` calls — is dead too,
/// and so is `deeper` below that.
#[test]
fn a_dead_chain_is_reported_to_its_end_in_one_run() {
    let analysis = analyze_fixture("reach");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem)
            .into_iter()
            .filter(|(file, _)| file == "app/src/cascade.rs")
            .map(|(_, name)| name)
            .collect::<Vec<_>>(),
        vec!["orphan", "helper", "deeper"]
    );
    // Only the head of the chain is unreferenced; the two below it are
    // referenced by something that is itself unreachable, which is the claim
    // reference counting could not make.
    assert_finding_message(
        &analysis,
        "orphan",
        "is never referenced by any resolved path",
    );
    for name in ["helper", "deeper"] {
        assert_finding_message(
            &analysis,
            name,
            "is referenced only from items that nothing reaches",
        );
    }
}

/// The case no number of reruns ever finds: `ping` and `pong` name each other
/// and nothing names either, so both are referenced and both are dead. Each is
/// reported separately, because each is separately deletable and a group
/// finding would need a name the baseline could key on.
#[test]
fn a_mutually_recursive_dead_pair_is_reported_in_full() {
    let analysis = analyze_fixture("reach");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem)
            .into_iter()
            .filter(|(file, _)| file == "app/src/cycle.rs")
            .collect::<Vec<_>>(),
        vec![
            ("app/src/cycle.rs".to_string(), "ping"),
            ("app/src/cycle.rs".to_string(), "pong"),
        ]
    );
}

/// The finding reachability must not invent: `main` reaches `start`, `start`
/// reaches `middle`, `middle` reaches `leaf`, and every one of them stays
/// quiet.
#[test]
fn a_live_chain_from_an_entry_point_stays_quiet() {
    let analysis = analyze_fixture("reach");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file == Path::new("app/src/live.rs")),
        "an entry point carries all the way down: {:?}",
        analysis.findings
    );
}

/// An opaque mention is a root rather than an edge. `mentioned` is named only
/// from inside macro input, and that input sits in a function nothing reaches
/// — the one case where reading the mention as an ordinary reference would
/// build a false positive out of something we had already admitted we could
/// not read.
#[test]
fn an_opaque_mention_keeps_its_target_alive_when_every_referrer_is_dead() {
    let analysis = analyze_fixture("reach");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem)
            .into_iter()
            .filter(|(file, _)| file == "app/src/opaque.rs")
            .map(|(_, name)| name)
            .collect::<Vec<_>>(),
        vec!["dead_caller"],
        "the caller is dead; what it mentions through a macro is not"
    );
}

/// A library's public surface is a root, because consumers Deadwood cannot see
/// call it — declared outright by `[public-api]`, or by being `pub` under
/// `pub` modules from the crate root. Either way what the surface calls stays
/// quiet, which is the difference between a usable report on a library and a
/// page of noise.
#[test]
fn a_librarys_public_surface_keeps_what_it_calls_alive() {
    let analysis = analyze_fixture("reach");

    // `entry` is on the surface and nothing in the workspace names it, which
    // is the advisory finding Deadwood has always made. Being a root does not
    // exempt it — it exempts what it *calls*.
    assert_finding_message(
        &analysis,
        "entry",
        "is never referenced by any resolved path",
    );
    for name in ["worker", "detail"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(name)),
            "`{name}` is reached through the surface: {:?}",
            analysis.findings
        );
    }

    // `hidden` is the half the surface rule cannot infer: a private module, so
    // `plugin` is an ordinary node and `support` falls with it.
    assert_finding_message(
        &analysis,
        "plugin",
        "is never referenced by any resolved path",
    );
    assert_finding_message(
        &analysis,
        "support",
        "is referenced only from items that nothing reaches",
    );

    // Declaring both surfaces silences the items themselves and everything
    // they call — including `support`, whose only claim to being reached is
    // the `[public-api]` listing on `plugin`.
    let declared = analyze_configured("reach", "public-api.toml");
    assert!(
        !declared
            .findings
            .iter()
            .any(|f| f.file.starts_with("declared/")),
        "a declared API and its callees are quiet: {:?}",
        declared.findings
    );
}

/// `mod inner; pub use inner::*;` puts `inner`'s items on a library's public
/// surface, and the root set follows the glob there
/// ([#25](https://github.com/rlorenzo/deadwood/issues/25)). Rooting *removes*
/// findings, so the assertions that matter most here are the ones that must
/// still be made.
#[test]
fn a_pub_use_glob_puts_what_it_re_exports_on_the_public_surface() {
    let analysis = analyze_fixture("globs");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // Under a module no glob exports: the plain `use crate::imported::*;`
            // in `other` is an import, and an import re-exports nothing.
            ("facade/src/imported/buried.rs".to_string(), "Stale"),
            ("facade/src/imported.rs".to_string(), "from_import"),
            // `deep` is `pub(crate)`, so the descent under the glob stops
            // above it however `pub` the item in it is.
            ("facade/src/inner/deep.rs".to_string(), "buried"),
            // The half rooting must not move: behind the glob, so a consumer
            // can name it, and reported anyway because nothing does.
            ("facade/src/inner.rs".to_string(), "never_named"),
            ("facade/src/other.rs".to_string(), "helper"),
            // A binary has no surface for the glob in `main.rs` to reach.
            ("tool/src/dead_end.rs".to_string(), "caller"),
            ("tool/src/hidden.rs".to_string(), "from_glob"),
        ],
    );

    // The four findings this phase removed, and every one of them was a claim
    // about something a consumer can write: `facade::thing`,
    // `facade::nested::deeper`, and `facade::Carried` through the re-export
    // that carries it — which is itself no longer reported, for the same
    // reason a `pub use` in `lib.rs` never was.
    for name in ["thing", "deeper", "Carried"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(name)),
            "`{name}` is nameable through the glob: {:?}",
            analysis.findings
        );
    }

    // `hub` re-exports `facade` across the workspace and roots nothing new:
    // the only modules of another member a path can name are `pub` from that
    // member's own crate root, which the surface rule already covered — so
    // `imported` and `inner::deep` keep the findings above.
    assert_eq!(
        reported(&analysis, FindingKind::UnusedReexport),
        vec![("facade/src/imported.rs".to_string(), "Stale")],
    );
    assert_finding_message(
        &analysis,
        "from_import",
        "is referenced only from items that nothing reaches",
    );
}

/// Paths that cross workspace members resolve by crate name, with cargo's
/// dash-to-underscore normalization (`engine-core` is `engine_core` in a
/// path). Items the other member never reaches are still reported.
#[test]
fn paths_resolve_across_workspace_members() {
    let analysis = analyze_fixture("crosscrate");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            ("engine-core/src/api.rs".to_string(), "Unused"),
            ("engine-core/src/lib.rs".to_string(), "never_started"),
        ],
        "`start` and `Handle` are reached from `app`, and `aliased_only` from \
         `aliased` through the `motor` dependency rename; none may be reported"
    );
}

/// Dependencies a package declares but never names are reported, and every
/// channel through which one *can* be named keeps its entry alive: plain
/// paths, a rename, a re-export, `extern crate`, macro bodies, attribute
/// paths and attribute strings, a doc example, a test target, a build script,
/// a file only a macro expansion declares, and the `[features]` table.
/// Feature- and platform-gated entries are judged like any other under the
/// default `cfg` matrix, which analyzes the code behind those gates.
#[test]
fn unused_dependencies_are_reported_and_every_reference_channel_counts() {
    let analysis = analyze_fixture("deps");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        vec![
            // Optional, and named by nothing: judgeable since the default
            // matrix compiles every `#[cfg(feature = ...)]` branch.
            ("Cargo.toml".to_string(), "optional_crate"),
            // `[dependencies]`: named nowhere in the package.
            ("Cargo.toml".to_string(), "unused_crate"),
            // `[dev-dependencies]`: the other one is used by `tests/it.rs`.
            ("Cargo.toml".to_string(), "unused_dev_crate"),
            // `[build-dependencies]`: the other one is used by `build.rs`.
            ("Cargo.toml".to_string(), "unused_build_crate"),
            // `[target.'cfg(unix)'.dependencies]`, judged for the same reason.
            ("Cargo.toml".to_string(), "platform_crate"),
        ],
        "every other entry is named somewhere Deadwood has to look"
    );

    // Two channels with no reachable code behind them at all: a file that
    // only `automod::dir!` declares, and a `[features]` entry forwarding to a
    // dependency. Both were false positives before they were closed.
    for entry in ["regression_only_crate", "feature_only_crate"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is named where module resolution cannot see it: {:?}",
            analysis.findings
        );
    }

    // The rename is reported (and matched) by the manifest key, not by the
    // package name behind it.
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("engine-widget")),
        "a renamed entry is judged by its alias: {:?}",
        analysis.findings
    );

    // Gating an entry is not what keeps it alive; being named is. Both of
    // these are used only from behind the very `cfg` that gates them.
    for entry in ["gated_used_crate", "platform_used_crate"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is named behind its own gate, which the default matrix \
             compiles: {:?}",
            analysis.findings
        );
    }

    assert!(
        !reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "the unused-pub check still runs alongside: {:?}",
        analysis.warnings
    );
}

/// `[target.'cfg(any())'.dependencies]` is the idiom for an entry that exists
/// to constrain version resolution and is deliberately compiled by no target
/// (serde pins `serde_derive` this way). No code can name it, and reporting it
/// would be a false positive, so it is skipped with a warning that says which
/// of the two it is.
#[test]
fn a_dependency_no_target_ever_builds_is_skipped_rather_than_reported() {
    let fixture = fixtures().join("cfgdeps");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("pinned_crate")),
        "a never-built entry is not a finding: {:?}",
        analysis.findings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("`pinned_crate`") && w.contains("holds on no target at all")),
        "the skip must name the reason: {:?}",
        analysis.warnings
    );
}

/// Path dependencies between workspace members are dependencies like any
/// other: `app` uses `engine_core::`, `aliased` uses the `motor` rename, and
/// neither entry may be reported.
#[test]
fn dependencies_between_workspace_members_are_seen() {
    let analysis = analyze_fixture("crosscrate");
    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        Vec::new(),
        "both members name the crate they depend on"
    );
}

/// A dependency renamed in `Cargo.toml` (`motor = { package = "engine-core" }`)
/// is spelled by its alias in code. The alias is derivable from neither the
/// package name nor the lib target name, so without reading it out of the
/// manifest the path resolves to nothing and everything it reaches looks dead.
#[test]
fn paths_through_a_dependency_rename_resolve() {
    let analysis = analyze_fixture("crosscrate");

    let unused = reported(&analysis, FindingKind::UnusedPubItem);
    assert!(
        !unused.iter().any(|(_, name)| *name == "aliased_only"),
        "`aliased_only` is called as `motor::aliased_only()`: {unused:?}"
    );
    assert!(
        !unused.iter().any(|(_, name)| *name == "Handle"),
        "`Handle` is named through the alias as `motor::api::Handle`: {unused:?}"
    );
}

// -- cfg evaluation --------------------------------------------------------
//
// The `cfggates` fixture gives every module one unreferenced `pub fn`, so
// "was this analyzed?" reads straight off the unused-pub findings: the item is
// there when the module is part of the analyzed build and gone when it is not.

/// Under the default matrix — every feature, every target, tests included —
/// every gate that can hold anywhere is followed, exactly as before this phase.
/// The one thing that is new is the finding about the gate that cannot.
#[test]
fn the_default_matrix_follows_every_gate_and_reports_the_ones_that_can_never_hold() {
    let analysis = analyze_fixture("cfggates");

    assert_eq!(
        reported(&analysis, FindingKind::UnsatisfiableCfg),
        vec![
            // An item-level gate, `all` with one arm no manifest can satisfy.
            ("src/declared_feature.rs".to_string(), "never_built"),
            // A file-level `#![cfg(...)]`, which gates every item below it.
            ("src/inner_impossible.rs".to_string(), ""),
            // A `mod` behind a feature that does not exist.
            ("src/lib.rs".to_string(), "missing_feature"),
        ],
        "only gates no build can satisfy are reported: {:?}",
        analysis.findings
    );
    let gate = analysis
        .findings
        .iter()
        .find(|f| {
            f.kind == FindingKind::UnsatisfiableCfg && f.name.as_deref() == Some("missing_feature")
        })
        .expect("the mod gate is reported");
    assert_eq!(gate.line, Some(15), "the line is the `#[cfg]` attribute's");
    assert!(
        gate.message
            .contains("`#[cfg(feature = \"gone\")]` can never hold")
            && gate.message.contains("declares no feature `gone`"),
        "the message must name the gate and the missing feature: {}",
        gate.message
    );

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // Behind a feature the manifest declares: followed.
            (
                "src/declared_feature.rs".to_string(),
                "from_declared_feature"
            ),
            // Gated by an inner `#![cfg(...)]` rather than one on the `mod`.
            ("src/inner_gated.rs".to_string(), "from_inner_gate"),
            // Behind the impossible gates: reported *and* still followed, so
            // that adding a finding kind never moves the other detectors.
            (
                "src/inner_impossible.rs".to_string(),
                "from_inner_impossible",
            ),
            ("src/missing_feature.rs".to_string(), "from_missing_feature"),
            // Platform-gated, and every platform is possible by default.
            ("src/on_windows.rs".to_string(), "from_windows"),
            // `any(<unevaluable>, <impossible>)` is not impossible.
            (
                "src/partly_unevaluable.rs".to_string(),
                "from_partly_unevaluable"
            ),
            // A `cfg` we do not model at all.
            ("src/unevaluable.rs".to_string(), "from_unevaluable"),
        ],
        "every module was analyzed: {:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::DeadFile).is_empty(),
        "a gated file is never a dead file: {:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("used_by_tests_only")),
        "`#[cfg(test)]` code counts as a use by default: {:?}",
        analysis.findings
    );
}

/// Narrowing the targets takes the platform modules out of the analyzed build
/// — the one gated on its `mod` declaration and the one gated by an inner
/// `#![cfg(...)]` alike. The load-bearing half is the first assertion: code
/// that is not in this build must not turn into a dead file, or narrowing the
/// matrix would trade one false positive for another.
#[test]
fn a_narrowed_target_matrix_excludes_a_platform_module_without_calling_it_dead() {
    let analysis = analyze_configured("cfggates", "linux-only.toml");

    for excluded in ["src/on_windows.rs", "src/inner_gated.rs"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.file.starts_with(excluded)),
            "`{excluded}` is neither analyzed nor reported: {:?}",
            analysis.findings
        );
    }
    // Everything else is untouched, including the gates we cannot evaluate.
    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            (
                "src/declared_feature.rs".to_string(),
                "from_declared_feature"
            ),
            (
                "src/inner_impossible.rs".to_string(),
                "from_inner_impossible",
            ),
            ("src/missing_feature.rs".to_string(), "from_missing_feature"),
            (
                "src/partly_unevaluable.rs".to_string(),
                "from_partly_unevaluable"
            ),
            ("src/unevaluable.rs".to_string(), "from_unevaluable"),
        ]
    );
}

/// The `cfg(test)` decision, pinned from both sides. Test code counts as a use
/// by default — the quiet answer, and the one that keeps an absent config a
/// no-op — and `test = false` is how a project asks the other question.
#[test]
fn test_code_counts_as_a_use_until_the_matrix_says_otherwise() {
    let baseline = analyze_fixture("cfggates");
    assert!(
        !baseline
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("used_by_tests_only")),
        "the default must be the quiet one: {:?}",
        baseline.findings
    );

    let analysis = analyze_configured("cfggates", "no-tests.toml");
    assert!(
        analysis
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::UnusedPubItem
                && f.name.as_deref() == Some("used_by_tests_only")),
        "with the test build out of the matrix, a test-only helper is unused: {:?}",
        analysis.findings
    );
}

/// Narrowing the features leaves a declared-feature module out of the build,
/// and takes the optional dependencies nothing can enable with it — those go
/// back to being unjudgeable, out loud.
#[test]
fn a_narrowed_feature_matrix_excludes_gated_code_and_its_optional_dependencies() {
    let fixture = fixtures().join("cfggates");
    let analysis = analyze(&fixture, Some(&fixture.join("features-off.toml")))
        .expect("`features-off.toml` should load");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file.starts_with("src/declared_feature.rs")),
        "`extra` is off, so nothing in that module is in the build: {:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "an optional entry no feature can enable is not judgeable: {:?}",
        analysis.findings
    );
    assert!(
        analysis
            .warnings
            .iter()
            .any(|w| w.contains("`optional_helper`") && w.contains("[cfg] features")),
        "the skip must name the matrix as the reason: {:?}",
        analysis.warnings
    );
    // The gate that can never hold is judged against every build there could
    // be, not against the configured one, so narrowing does not silence it.
    assert_eq!(
        reported(&analysis, FindingKind::UnsatisfiableCfg),
        vec![
            ("src/inner_impossible.rs".to_string(), ""),
            ("src/lib.rs".to_string(), "missing_feature")
        ]
    );
}

/// The new kind is a first-class one: `[severity]` reaches it by its serde tag
/// with no plumbing of its own, and it carries that tag into the JSON.
#[test]
fn the_new_finding_kind_is_configurable_and_typed() {
    let fixture = fixtures().join("cfggates");
    let analysis =
        analyze(&fixture, Some(&fixture.join("cfg-off.toml"))).expect("`cfg-off.toml` should load");
    assert!(
        reported(&analysis, FindingKind::UnsatisfiableCfg).is_empty(),
        "`unsatisfiable_cfg = \"off\"` removes the finding entirely: {:?}",
        analysis.findings
    );

    let reported = analyze_fixture("cfggates");
    let json = deadwood::report::render_json(&reported).expect("the report should render");
    assert!(
        json.contains("\"kind\": \"unsatisfiable_cfg\""),
        "the JSON carries the same tag the config file keys off:\n{json}"
    );
    let text = deadwood::report::render_text(&reported);
    assert!(
        text.contains("Unsatisfiable cfg gates:\n"),
        "and the text report gives it a group of its own:\n{text}"
    );
}

// -- configuration ---------------------------------------------------------
//
// The `config` fixture produces at least one finding of every kind with no
// configuration at all. Each test below applies one setting and asserts what
// it removed from that baseline, and — just as important — what it left.

/// Names of every finding, kind by kind, so a config's effect can be stated as
/// a difference from the unconfigured run.
fn all_reported(analysis: &Analysis) -> Vec<(FindingKind, String, String)> {
    analysis
        .findings
        .iter()
        .map(|f| {
            (
                f.kind,
                f.file.display().to_string(),
                f.name.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The baseline the rest of this section is measured against, and the property
/// the whole feature is built around: with no config file, nothing changes.
/// Every finding is `deny`, exactly as before severity existed.
#[test]
fn a_workspace_with_no_config_file_behaves_exactly_as_before() {
    let fixture = fixtures().join("config");
    assert!(
        !fixture.join("deadwood.toml").exists(),
        "the baseline fixture must have no discoverable config"
    );

    let analysis = analyze_fixture("config");
    assert_eq!(
        all_reported(&analysis)
            .into_iter()
            .map(|(kind, file, name)| (kind, format!("{file}:{name}")))
            .collect::<Vec<_>>(),
        vec![
            (
                FindingKind::UnusedDependency,
                "app/Cargo.toml:stale_crate".into()
            ),
            (
                FindingKind::UnusedDependency,
                "app/Cargo.toml:vendored_native".into()
            ),
            (
                FindingKind::UnusedDependency,
                "surface/Cargo.toml:sidecar".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/api.rs:public_entry".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/generated.rs:generated_thing".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/generated.rs:call_helper".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/internal/hidden.rs:Buried".into()
            ),
            (
                FindingKind::UnusedReexport,
                "surface/src/internal.rs:Buried".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/internal.rs:internal_leftover".into()
            ),
            (
                FindingKind::UnusedPubItem,
                "surface/src/lib.rs:another_entry".into()
            ),
            (FindingKind::DeadFile, "surface/src/orphan.rs:".into()),
        ]
    );
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Deny),
        "every kind defaults to deny: {:?}",
        analysis.findings
    );
    assert!(
        analysis.has_denied(),
        "an unconfigured run with findings fails"
    );
}

/// `ignore` removes findings *about* the matched files, and nothing else. The
/// second assertion is the load-bearing one: `generated.rs` holds the only
/// call to `api::helper`, so dropping its references along with its findings
/// would invent a false positive next door — turning every `ignore` entry into
/// a liability.
#[test]
fn ignore_patterns_suppress_findings_without_dropping_references() {
    let analysis = analyze_configured("config", "ignore.toml");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.file.starts_with("surface/src/generated.rs")
                || f.file.starts_with("surface/src/orphan.rs")),
        "no finding may be reported about an ignored file: {:?}",
        analysis.findings
    );
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("helper")),
        "an ignored file's references still count: {:?}",
        analysis.findings
    );
    // Everything outside the ignored files is untouched.
    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            ("surface/src/api.rs".to_string(), "public_entry"),
            ("surface/src/internal/hidden.rs".to_string(), "Buried"),
            ("surface/src/internal.rs".to_string(), "internal_leftover"),
            ("surface/src/lib.rs".to_string(), "another_entry"),
        ]
    );
}

/// The one place `ignore` has to reach inside module resolution. A live file
/// declares `mod generated;` for a file a build step would have written; it is
/// not there. Unignored, that is an unresolved module, which skips every check
/// for the package — so an ignored file would silence the code around it.
///
/// This fixture also pins config *discovery*: its `deadwood.toml` is found by
/// walking up, not passed with `--config`.
#[test]
fn an_ignored_module_declaration_does_not_skip_the_package() {
    let discovered = analyze_fixture("ignoremod");
    assert!(
        discovered.warnings.is_empty(),
        "an ignored module is not an unresolved one: {:?}",
        discovered.warnings
    );
    assert_eq!(
        reported(&discovered, FindingKind::UnusedPubItem),
        vec![("src/lib.rs".to_string(), "leftover")],
        "the rest of the package must still be checked"
    );

    // The contrast, with a config that sets nothing: the declaration is an
    // unresolved module again, and every check for the package stands down.
    let fixture = fixtures().join("ignoremod");
    let unconfigured = analyze(&fixture, Some(&fixture.join("empty.toml"))).unwrap();
    assert!(
        unconfigured.findings.is_empty(),
        "incomplete resolution must report nothing: {:?}",
        unconfigured.findings
    );
    assert!(
        unconfigured
            .warnings
            .iter()
            .any(|w| w.contains("has no file at")),
        "the unresolved module must be surfaced: {:?}",
        unconfigured.warnings
    );
}

/// `warn` keeps every finding visible and stops it failing the run. That split
/// is the point: a project can adopt a check before it is clean.
#[test]
fn warn_severity_reports_everything_and_fails_nothing() {
    let baseline = analyze_fixture("config");
    let analysis = analyze_configured("config", "severity-warn.toml");

    assert_eq!(
        all_reported(&analysis),
        all_reported(&baseline),
        "`warn` changes no finding, only what it costs"
    );
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Warn),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis.has_denied(),
        "a run with only `warn` findings succeeds"
    );

    let text = deadwood::report::render_text(&analysis);
    assert!(text.contains("Dead files (warn):"), "{text}");
    assert!(text.contains("0 deny, 11 warn"), "{text}");
}

/// `off` is stronger than `warn`: the finding never exists, so it is absent
/// from the text, the JSON, and the count.
#[test]
fn off_severity_removes_findings_entirely() {
    let analysis = analyze_configured("config", "severity-off.toml");

    assert!(
        reported(&analysis, FindingKind::DeadFile).is_empty(),
        "{:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "{:?}",
        analysis.findings
    );
    // Kinds the file does not mention keep the default.
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Deny),
        "{:?}",
        analysis.findings
    );
    assert!(analysis.has_denied());

    let text = deadwood::report::render_text(&analysis);
    assert!(!text.contains("Dead files"), "{text}");
    assert!(
        !text.contains("deny,"),
        "with nothing downgraded the summary keeps its original shape:\n{text}"
    );
}

/// Severity is per kind, so a run can carry both at once — and one surviving
/// `deny` finding is enough to fail it.
#[test]
fn a_single_deny_finding_fails_a_run_full_of_warnings() {
    let analysis = analyze_configured("config", "severity-mixed.toml");

    let dead_file = analysis
        .findings
        .iter()
        .find(|f| f.kind == FindingKind::DeadFile)
        .expect("the dead file is downgraded, not removed");
    assert_eq!(dead_file.severity, Severity::Warn);
    assert!(
        reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "{:?}",
        analysis.findings
    );
    assert!(
        analysis.has_denied(),
        "the re-export and dependency findings are still deny: {:?}",
        analysis.findings
    );
}

/// The noise lever the whole setting exists for: one line saying "this crate's
/// surface is the API" instead of an entry per item.
#[test]
fn a_public_api_crate_listing_silences_that_crates_surface() {
    let analysis = analyze_configured("config", "public-api-crate.toml");

    assert!(
        reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "the listed crate's pub items are declared API: {:?}",
        analysis.findings
    );
    assert!(
        reported(&analysis, FindingKind::UnusedReexport).is_empty(),
        "a re-export is surface too: {:?}",
        analysis.findings
    );
    // The other detectors are untouched — this is not a blanket mute.
    assert_eq!(
        reported(&analysis, FindingKind::DeadFile),
        vec![("surface/src/orphan.rs".to_string(), "")]
    );
    assert_eq!(reported(&analysis, FindingKind::UnusedDependency).len(), 3);
}

/// An `items` glob covers what it names and no more, so a project can declare
/// one module its API without losing the check everywhere else.
#[test]
fn public_api_item_globs_cover_only_what_they_name() {
    let analysis = analyze_configured("config", "public-api-items.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // `surface::api::*` covers `public_entry`, and nothing below.
            ("surface/src/generated.rs".to_string(), "generated_thing"),
            ("surface/src/generated.rs".to_string(), "call_helper"),
            ("surface/src/internal/hidden.rs".to_string(), "Buried"),
            ("surface/src/internal.rs".to_string(), "internal_leftover"),
            ("surface/src/lib.rs".to_string(), "another_entry"),
        ],
        "a single-segment `*` must not swallow other modules or the crate root"
    );
}

/// Closes #9: entries that are load bearing without being named by any code.
/// Workspace-wide and per-package lists both apply, and neither reaches an
/// entry it does not name.
#[test]
fn allowlisted_dependency_entries_are_never_judged() {
    let analysis = analyze_configured("config", "deps-allow.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        vec![("app/Cargo.toml".to_string(), "stale_crate")],
        "`sidecar` is allowed workspace-wide and `vendored_native` in `app`; \
         the unlisted entry in the same manifest stays reported"
    );
}

/// A configuration that does not do what it says is worse than none, so a
/// misspelling stops the run with a message naming the file, the key, and the
/// keys that do exist.
#[test]
fn a_malformed_config_fails_the_run_with_an_actionable_message() {
    let fixture = fixtures().join("config");
    let err = analyze(&fixture, Some(&fixture.join("broken.toml")))
        .expect_err("a config error must not be survivable");
    let message = format!("{err:#}");

    assert!(message.contains("broken.toml"), "{message}");
    assert!(message.contains("unknown field `ignor`"), "{message}");
    assert!(message.contains("`ignore`"), "{message}");
}

/// A `--config` naming a file that is not there is an error too: silently
/// falling back to the defaults would apply the wrong settings to a whole CI
/// run, and the run would pass while checking something else.
#[test]
fn a_missing_config_file_is_an_error_rather_than_a_silent_fallback() {
    let fixture = fixtures().join("config");
    let err = analyze(&fixture, Some(&fixture.join("no-such-file.toml")))
        .expect_err("a named config file must exist");
    assert!(
        format!("{err:#}").contains("could not read config file"),
        "{err:#}"
    );
}

/// The exit code is the whole interface for CI, so it is pinned on the binary
/// rather than inferred from the library: 0 clean or advisory, 1 denied,
/// 2 configuration error.
#[test]
fn exit_codes_follow_severity() {
    let run = |config: Option<&str>| {
        let fixture = fixtures().join("config");
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"));
        command.arg("check").arg(&fixture);
        if let Some(config) = config {
            command.arg("--config").arg(fixture.join(config));
        }
        let output = command.output().expect("the binary should run");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
        )
    };

    let (code, stdout) = run(None);
    assert_eq!(code, Some(1), "unconfigured findings fail the run");
    assert!(stdout.contains("11 finding(s)"), "{stdout}");

    let (code, stdout) = run(Some("severity-warn.toml"));
    assert_eq!(code, Some(0), "advisory findings do not fail the run");
    assert!(
        stdout.contains("Unused public items (warn):"),
        "and are still printed, marked:\n{stdout}"
    );

    let (code, _) = run(Some("severity-mixed.toml"));
    assert_eq!(code, Some(1), "one surviving deny finding fails the run");

    let (code, stdout) = run(Some("broken.toml"));
    assert_eq!(code, Some(2), "a config error is an error, not a finding");
    assert!(stdout.is_empty(), "no report is produced: {stdout}");
}

/// The five cases that decide whether the misplaced-dependency check is worth
/// shipping, each pinned by one entry of the `depkinds` fixture. Two of them
/// are findings; the other three are the false positives a naive per-target
/// split would produce.
#[test]
fn dependencies_declared_in_the_wrong_table_are_reported() {
    let analysis = analyze_fixture("depkinds");

    let mut names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        // A normal entry only `tests/it.rs` names, one only `examples/demo.rs`
        // names — both link `[dev-dependencies]` — one only the out-of-line
        // body of a `#[cfg(test)] mod` names, and a build-dependency the build
        // script never touches.
        vec![
            "example_only_crate",
            "outline_test_crate",
            "stale_build_crate",
            "test_only_crate",
        ],
        "{:?}",
        analysis.findings
    );

    for entry in [
        // Used by the lib *and* the test: `[dependencies]` is where it belongs.
        "shared_crate",
        // A dev-dependency used by a `#[cfg(test)]` module inside the lib. The
        // module is part of the lib target and links dev-dependencies anyway,
        // and this is the largest false-positive source the check has.
        "cfg_test_crate",
        // A dev-dependency named only by a doc example. Doctests link
        // dev-dependencies, and a word in a doc comment names no target at all.
        "doc_only_crate",
        // A build-dependency the build script names.
        "build_crate",
        // Named by a file three declarations reach, one of them ungated: the
        // library genuinely uses it, whichever declaration is read first.
        "shared_view_crate",
        // The same, one level further down, through a child declaration that
        // carries no gate of its own.
        "shared_view_child_crate",
    ] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is declared in the table its references can see: {:?}",
            analysis.findings
        );
    }

    // The other dependency check answers a different question, and this
    // fixture gives it nothing to say: every entry is named by something.
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "no entry here is unreferenced: {:?}",
        analysis.findings
    );
}

/// Both halves of the finding have to be actionable: the table the entry sits
/// in and the table it belongs in, in text and in JSON, under a kind
/// `[severity]` reaches by its own serde tag.
#[test]
fn a_misplaced_dependency_names_both_tables_and_is_configurable() {
    let analysis = analyze_fixture("depkinds");

    let moved_down = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some("test_only_crate"))
        .expect("the test-only entry is reported");
    assert!(
        moved_down
            .message
            .contains("belongs in `[dev-dependencies]`")
            && moved_down.message.contains("rather than `[dependencies]`"),
        "{}",
        moved_down.message
    );
    assert_eq!(moved_down.file, PathBuf::from("Cargo.toml"));

    let stale_build = analysis
        .findings
        .iter()
        .find(|f| f.name.as_deref() == Some("stale_build_crate"))
        .expect("the stale build entry is reported");
    assert!(
        stale_build.message.contains("belongs in `[dependencies]`")
            && stale_build
                .message
                .contains("rather than `[build-dependencies]`"),
        "{}",
        stale_build.message
    );

    let text = deadwood::report::render_text(&analysis);
    assert!(text.contains("Misplaced dependencies:\n"), "{text}");

    let json: serde_json::Value =
        serde_json::from_str(&deadwood::report::render_json(&analysis).unwrap()).unwrap();
    assert!(
        json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "misplaced_dependency" && f["name"] == "test_only_crate"),
        "{json}"
    );

    let silenced = analyze_configured("depkinds", "off.toml");
    assert!(
        reported(&silenced, FindingKind::MisplacedDependency).is_empty(),
        "`misplaced_dependency = \"off\"` removes the finding entirely: {:?}",
        silenced.findings
    );
}

/// A `#[cfg(test)] mod tests;` whose body lives in its own file is unit-test
/// code exactly as the inline form is, and the gate saying so is written in
/// the parent — so nothing in the file itself can be read to find it out.
///
/// The other direction is the one that must not move: a file some *ungated*
/// declaration also reaches is library code, however many gated declarations
/// name it too. Getting that backwards would report a `[dependencies]` entry
/// the library genuinely uses as belonging in `[dev-dependencies]` — a false
/// positive, where the version before this was a missed finding.
#[test]
fn a_test_only_module_in_its_own_file_is_still_test_code() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert!(
        reported_names.contains(&"outline_test_crate"),
        "`src/outline_tests.rs` is reached only by `#[cfg(test)] mod outline_tests;`, \
         so what it names is a dev-dependency: {:?}",
        analysis.findings
    );
    for entry in ["shared_view_crate", "shared_view_child_crate"] {
        assert!(
            !reported_names.contains(&entry),
            "`{entry}` is named by a file an ungated declaration reaches: {:?}",
            analysis.findings
        );
    }

    // The files themselves are ordinary reachable files either way: a gate on
    // the declaration decides which code a file *is*, never whether it is
    // dead.
    assert!(
        reported(&analysis, FindingKind::DeadFile).is_empty(),
        "every file here is declared by something: {:?}",
        analysis.findings
    );
}

// -- the baseline file -----------------------------------------------------
//
// The `baseline` fixture produces six findings with no configuration at all,
// and every `*.toml` beside it names a `*-baseline.json` that disagrees with
// those six in exactly one controlled way.

/// Baselined findings as `(kind, file, name)`, which is the match key itself.
fn stale_keys(analysis: &Analysis) -> Vec<String> {
    analysis
        .baseline
        .as_ref()
        .map(|baseline| baseline.stale.iter().map(|key| key.describe()).collect())
        .unwrap_or_default()
}

fn suppressed(analysis: &Analysis) -> usize {
    analysis
        .baseline
        .as_ref()
        .map(|baseline| baseline.suppressed)
        .unwrap_or(0)
}

/// A copy of the `baseline` fixture in a scratch directory, so a test may
/// write to it. The fixture itself is never modified by any test.
fn scratch_fixture(test: &str) -> PathBuf {
    let dir = scratch_copy("baseline", test);
    // The `*.toml` config files beside the fixture each name a baseline of
    // their own. These tests run without `--config` and against the default
    // location, so the copies come along only to confuse a future reader.
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name != "Cargo.toml" && (name.ends_with(".toml") || name.ends_with("-baseline.json")) {
            std::fs::remove_file(&path).unwrap();
        }
    }
    dir
}

/// A copy of a fixture in a scratch directory, for the runs that write.
fn scratch_copy(fixture: &str, test: &str) -> PathBuf {
    fn copy_into(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_into(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    let dir =
        std::env::temp_dir().join(format!("deadwood-{fixture}-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_into(&fixtures().join(fixture), &dir);
    dir
}

fn run_binary(dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_deadwood"))
        .arg("check")
        .arg(dir)
        .args(args)
        .output()
        .expect("the binary should run");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

/// The whole point of the feature: a project with day-one findings records
/// them once and its runs go quiet, without a single finding being fixed.
#[test]
fn a_baseline_suppresses_exactly_the_findings_it_records() {
    let unconfigured = analyze_fixture("baseline");
    assert_eq!(
        unconfigured.findings.len(),
        6,
        "{:?}",
        unconfigured.findings
    );
    assert!(
        unconfigured.baseline.is_none(),
        "no `baseline` key and no file at the default location is no baseline"
    );

    let analysis = analyze_configured("baseline", "all.toml");
    assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    assert!(!analysis.has_denied(), "so the run exits 0");
    assert_eq!(suppressed(&analysis), 6);
    assert!(
        stale_keys(&analysis).is_empty(),
        "{:?}",
        stale_keys(&analysis)
    );
}

/// The other half: a finding the baseline does not cover still fails the run,
/// and is the only thing in the report.
#[test]
fn a_new_finding_fails_a_run_the_accepted_ones_do_not() {
    let analysis = analyze_configured("baseline", "partial.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![("src/lib.rs".to_string(), "accepted")],
        "only the unrecorded finding is reported: {:?}",
        analysis.findings
    );
    assert_eq!(analysis.findings.len(), 1, "{:?}", analysis.findings);
    assert!(analysis.has_denied(), "and it fails the run");
    assert_eq!(suppressed(&analysis), 5);
}

/// A baseline that only ever grows is an excuse rather than a ledger, so an
/// entry nothing matches is named — and does not fail the run, because the
/// exit code follows severity and a fixed finding has none.
#[test]
fn entries_that_no_longer_occur_are_reported_as_stale() {
    let analysis = analyze_configured("baseline", "stale.toml");

    assert_eq!(
        stale_keys(&analysis),
        vec![
            "src/deleted.rs: dead_file".to_string(),
            "src/lib.rs: unused_pub_item `removed`".to_string(),
        ]
    );
    assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    assert!(!analysis.has_denied(), "fixing code must not fail a run");

    let text = deadwood::report::render_text(&analysis);
    assert!(text.contains("--prune-baseline"), "{text}");
}

/// Line numbers drift with every edit above them. A baseline that expired
/// whenever someone added an import would be worse than no baseline at all.
#[test]
fn a_finding_that_moved_is_still_baselined() {
    let analysis = analyze_configured("baseline", "drift.toml");

    assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    assert_eq!(suppressed(&analysis), 6);
    assert!(
        stale_keys(&analysis).is_empty(),
        "and nothing reads as stale: {:?}",
        stale_keys(&analysis)
    );
}

/// `unused_dependency` and `misplaced_dependency` point at the same manifest,
/// name the same entry, and carry no line: the kind is the only thing keeping
/// them apart. This baseline records each of the fixture's two entries under
/// the *other* kind, so a key without the kind in it would silence both real
/// findings and report nothing stale.
#[test]
fn the_two_dependency_kinds_are_never_confused() {
    let analysis = analyze_configured("baseline", "kinds.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        vec![("Cargo.toml".to_string(), "unused_crate")]
    );
    assert_eq!(
        reported(&analysis, FindingKind::MisplacedDependency),
        vec![("Cargo.toml".to_string(), "test_only_crate")]
    );
    assert_eq!(
        stale_keys(&analysis),
        vec![
            "Cargo.toml: misplaced_dependency `unused_crate`".to_string(),
            "Cargo.toml: unused_dependency `test_only_crate`".to_string(),
        ],
        "and neither recorded claim matched anything"
    );
}

/// Two `pub fn twin` in one file share a kind, a file and a name, and differ
/// only in the line the key ignores. One entry covers both: we cannot say
/// which occurrence is new, so reporting one would point at a line that is
/// very likely baselined — a wrong finding rather than a missed one.
#[test]
fn one_entry_covers_every_finding_that_shares_its_key() {
    let analysis = analyze_configured("baseline", "collision.toml");

    assert!(
        reported(&analysis, FindingKind::UnusedPubItem)
            .iter()
            .all(|(_, name)| *name != "twin"),
        "both twins are suppressed by the one entry: {:?}",
        analysis.findings
    );
    assert_eq!(suppressed(&analysis), 2);
}

/// A baseline path that is written down and not there must never read as
/// "nothing is baselined": the run would be green for the wrong reason, and a
/// typo would silently disarm a CI gate.
#[test]
fn a_missing_baseline_file_is_an_error_rather_than_an_empty_one() {
    let fixture = fixtures().join("baseline");
    let err = format!(
        "{:#}",
        analyze(&fixture, Some(&fixture.join("missing.toml")))
            .expect_err("a named baseline must exist")
    );
    assert!(err.contains("could not read baseline file"), "{err}");
    assert!(err.contains("no-such-baseline.json"), "{err}");
    assert!(err.contains("--write-baseline"), "the fix is named: {err}");

    let (code, stdout) = run_binary(&fixture, &["--config", "missing.toml"]);
    assert_eq!(code, Some(2), "an error, not a finding and not a pass");
    assert!(stdout.is_empty(), "and no report is produced: {stdout}");
}

/// The reverse failure is worse still: an unreadable file read as "everything
/// is baselined" is a permanently green run.
#[test]
fn a_malformed_baseline_is_an_error_never_a_warning() {
    let fixture = fixtures().join("baseline");
    let err = format!(
        "{:#}",
        analyze(&fixture, Some(&fixture.join("malformed.toml")))
            .expect_err("a baseline that does not parse is not survivable")
    );
    assert!(err.contains("invalid baseline file"), "{err}");
    assert!(err.contains("malformed-baseline.json"), "{err}");
    assert!(err.contains("unknown field `lines`"), "{err}");
    assert!(err.contains("`line`"), "the near miss is named: {err}");
}

/// The default location's exemption is "nothing is there", not "no *file* is
/// there". A directory sitting where the baseline belongs is a
/// misconfiguration, and reading it as "this project has no baseline" is the
/// same silently-green run the named-path cases refuse.
#[test]
fn a_directory_at_the_default_baseline_path_is_an_error() {
    let dir = scratch_fixture("not-a-file");
    std::fs::create_dir(dir.join("deadwood-baseline.json")).unwrap();

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(code, Some(2), "not a pass and not a finding: {stdout}");

    let err = format!(
        "{:#}",
        analyze(&dir, None).expect_err("a directory is not a baseline")
    );
    assert!(err.contains("is not a file"), "{err}");
    assert!(err.contains("deadwood-baseline.json"), "{err}");
}

/// And the exemption still holds for the case it exists for: no baseline at
/// the default location is a project that has not adopted one, reported
/// exactly as a Deadwood without the feature would.
#[test]
fn no_baseline_at_the_default_location_stays_silent() {
    let dir = scratch_fixture("absent");
    assert!(!dir.join("deadwood-baseline.json").exists());

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(
        code,
        Some(1),
        "the fixture's own findings still fail: {stdout}"
    );
    // Not a bare "baseline" search: this fixture's package is *named*
    // `baseline`, so the word occurs in ordinary finding text. What must be
    // absent is the block itself.
    for marker in ["suppressed by baseline", "Stale baseline entries", "Wrote "] {
        assert!(
            !stdout.contains(marker),
            "no baseline block, but found `{marker}`: {stdout}"
        );
    }
}

/// The adoption round trip, end to end and through the binary: record, go
/// quiet, then break something and watch only that fail. The new item is
/// inserted *above* the recorded ones, so every baselined line moves too.
#[test]
fn writing_a_baseline_silences_the_next_run_but_not_the_next_finding() {
    let dir = scratch_fixture("round-trip");
    let baseline = dir.join("deadwood-baseline.json");

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(code, Some(1), "the fixture has findings to begin with");
    assert!(stdout.contains("6 finding(s)"), "{stdout}");
    assert!(
        !baseline.exists(),
        "and no run without the flag creates the file"
    );

    let (code, stdout) = run_binary(&dir, &["--write-baseline"]);
    assert_eq!(code, Some(0), "recording them clears the run");
    assert!(
        stdout.contains("Wrote 6 finding(s) to baseline"),
        "{stdout}"
    );
    assert!(baseline.is_file(), "at the default location");

    // No `--config` here: the file at the default location is found on its own.
    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(code, Some(0), "{stdout}");
    assert_eq!(
        stdout,
        "No issues found.\n6 finding(s) suppressed by baseline `deadwood-baseline.json`.\n"
    );

    let source = dir.join("src/lib.rs");
    let shifted = std::fs::read_to_string(&source).unwrap().replace(
        "pub fn accepted",
        "pub fn brand_new() {}\n\npub fn accepted",
    );
    std::fs::write(&source, shifted).unwrap();

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(code, Some(1), "the new finding fails the run");
    assert!(stdout.contains("1 finding(s) in workspace"), "{stdout}");
    assert!(stdout.contains("pub fn `brand_new`"), "{stdout}");
    assert!(
        stdout.contains("6 finding(s) suppressed"),
        "and the moved ones are still covered:\n{stdout}"
    );
}

/// Pruning is how the file shrinks: the entries that no longer occur go, and
/// nothing new arrives — an entry pruning added would make the flag a second
/// `--write-baseline` that silently accepts today's regressions.
#[test]
fn pruning_drops_the_stale_entries_and_records_nothing_new() {
    let dir = scratch_fixture("prune");
    let baseline = dir.join("deadwood-baseline.json");

    run_binary(&dir, &["--write-baseline"]);
    let source = dir.join("src/lib.rs");
    let edited = std::fs::read_to_string(&source)
        .unwrap()
        // `accepted` is fixed (its entry goes stale) and a new one appears.
        .replace("pub fn accepted() {}", "pub fn brand_new() {}");
    std::fs::write(&source, edited).unwrap();

    let (code, stdout) = run_binary(&dir, &["--prune-baseline"]);
    assert_eq!(code, Some(1), "the new finding still fails: {stdout}");
    assert!(stdout.contains("Pruned from baseline"), "{stdout}");
    assert!(stdout.contains("unused_pub_item `accepted`"), "{stdout}");
    assert!(stdout.contains("pub fn `brand_new`"), "{stdout}");

    let written = std::fs::read_to_string(&baseline).unwrap();
    assert!(
        !written.contains("accepted"),
        "the stale entry is gone:\n{written}"
    );
    assert!(
        !written.contains("brand_new"),
        "and the new finding was not quietly accepted:\n{written}"
    );
    assert!(
        written.contains("orphan.rs"),
        "the rest is untouched:\n{written}"
    );

    let (code, _) = run_binary(&dir, &[]);
    assert_eq!(code, Some(1), "which is why the next run still fails");
}

/// Writing is explicit. A run without a flag may read the file and must never
/// create or change it, or a CI job would quietly accept whatever it found.
#[test]
fn no_run_without_a_flag_modifies_the_baseline() {
    let dir = scratch_fixture("read-only");
    let baseline = dir.join("deadwood-baseline.json");

    run_binary(&dir, &["--write-baseline"]);
    let recorded = std::fs::read_to_string(&baseline).unwrap();

    let source = dir.join("src/lib.rs");
    let edited = std::fs::read_to_string(&source)
        .unwrap()
        .replace("pub fn accepted() {}", "pub fn brand_new() {}");
    std::fs::write(&source, edited).unwrap();

    let (code, _) = run_binary(&dir, &[]);
    assert_eq!(code, Some(1));
    assert_eq!(
        std::fs::read_to_string(&baseline).unwrap(),
        recorded,
        "a plain run neither records the new finding nor drops the stale one"
    );
}

/// The compatibility promise: with no `baseline` key and no file at the
/// default location, nothing about a run changes — not the findings, not the
/// text, and not one key of the JSON.
#[test]
fn a_run_without_a_baseline_is_unchanged_in_every_output() {
    for fixture in ["simple", "deps", "depkinds", "cfggates"] {
        let analysis = analyze(&fixtures().join(fixture), None).expect("analysis should succeed");
        assert!(
            analysis.baseline.is_none(),
            "`{fixture}` has no baseline: {:?}",
            analysis.baseline
        );

        let rendered = deadwood::report::render_json(&analysis).unwrap();
        let json: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            json.as_object().unwrap().keys().collect::<Vec<_>>(),
            // `serde_json::Value` sorts its keys; the rendered document keeps
            // the declaration order, and the assertion below covers that.
            vec!["findings", "warnings", "workspace_root"],
            "`{fixture}` grew a JSON key"
        );
        assert!(
            !rendered.contains("baseline"),
            "`{fixture}` renders a baseline it does not have:\n{rendered}"
        );
        assert!(
            !deadwood::report::render_text(&analysis).contains("baseline"),
            "`{fixture}` mentions a baseline it does not have"
        );
    }
}

/// The `baseline` key names a path, and a path has directories in it. Failing
/// `--write-baseline` until the user runs `mkdir` protects nobody: the path
/// came from their config file and the write came from an explicit flag.
#[test]
fn writing_a_baseline_creates_the_directory_its_path_names() {
    let dir = scratch_fixture("nested");
    // Discovered by walking up, and resolved against the file that names it —
    // exactly the arrangement the README documents.
    std::fs::write(
        dir.join("deadwood.toml"),
        "baseline = \".deadwood/baseline.json\"\n",
    )
    .unwrap();
    let nested = dir.join(".deadwood/baseline.json");

    let (code, stdout) = run_binary(&dir, &["--write-baseline"]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(nested.is_file(), "the directory is created with the file");

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("6 finding(s) suppressed by baseline `.deadwood/baseline.json`"),
        "and the next run reads it back:\n{stdout}"
    );
}

/// The baseline is written through a temporary and renamed into place, so an
/// interrupted write cannot leave a truncated file behind — this module treats
/// a malformed baseline as a hard error, which would turn a cancelled CI job
/// into a committed artifact only a hand edit recovers. Interruption itself is
/// not reproducible here; what is checkable is that the mechanism runs and
/// tidies up after itself, on both writing paths.
#[test]
fn writing_a_baseline_leaves_no_temporary_behind() {
    let dir = scratch_fixture("atomic");
    let baseline = dir.join("deadwood-baseline.json");
    let temporary = dir.join("deadwood-baseline.json.tmp");

    for flag in ["--write-baseline", "--prune-baseline"] {
        let (code, stdout) = run_binary(&dir, &[flag]);
        assert_eq!(code, Some(0), "{flag}:\n{stdout}");
        assert!(baseline.is_file(), "{flag} leaves the baseline in place");
        assert!(
            !temporary.exists(),
            "{flag} leaves no `{}` beside it",
            temporary.display()
        );
        // Whole and readable: the point of the rename is that a reader never
        // sees half a file.
        let text = std::fs::read_to_string(&baseline).unwrap();
        serde_json::from_str::<serde_json::Value>(&text)
            .unwrap_or_else(|err| panic!("{flag} wrote valid JSON: {err}\n{text}"));
    }
}

/// Pruning re-serializes, so it cannot promise the file's bytes. What it must
/// promise is the entries: every surviving one keeps all of its fields, and a
/// hand-abbreviated entry is not quietly expanded into something else.
#[test]
fn pruning_preserves_every_field_of_the_entries_it_keeps() {
    let dir = scratch_fixture("prune-fields");
    let baseline = dir.join("deadwood-baseline.json");
    // Hand-written: one full entry, one carrying only the key, and one stale.
    std::fs::write(
        &baseline,
        r#"{
  "findings": [
    { "kind": "dead_file", "severity": "warn", "file": "src/orphan.rs", "line": 1,
      "message": "hand written" },
    { "kind": "unused_pub_item", "file": "src/lib.rs", "name": "accepted" },
    { "kind": "dead_file", "file": "src/deleted.rs" }
  ]
}
"#,
    )
    .unwrap();

    let (_, stdout) = run_binary(&dir, &["--prune-baseline"]);
    assert!(stdout.contains("src/deleted.rs: dead_file"), "{stdout}");

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    let kept = written["findings"].as_array().unwrap();
    assert_eq!(kept.len(), 2, "only the stale entry goes: {written}");
    // Order is the file's, and every field written by hand survives — down to
    // a severity that disagrees with the configured one, which the matcher
    // ignores and the rewrite must not "correct".
    assert_eq!(kept[0]["file"], "src/orphan.rs");
    assert_eq!(kept[0]["severity"], "warn");
    assert_eq!(kept[0]["message"], "hand written");
    // And an entry that carried only the key still carries only the key.
    assert_eq!(
        kept[1].as_object().unwrap().keys().collect::<Vec<_>>(),
        vec!["file", "kind", "name"],
        "{written}"
    );
}

/// The `scopes` fixture is written so that every `pub` item is either
/// shadowed by a binding — and so genuinely unreferenced — or named exactly
/// once, from inside the scope where a binding of that name is live. So the
/// list below is the whole claim: these four are shadowed, and the other nine
/// survive a binding that must not silence them.
///
/// The `assert_eq!` carries most of that weight, because it pins the *whole*
/// finding vector: an item wrongly reported fails it whether or not the loop
/// below names that item. The loop is there to say which survivor proves
/// which rule. Eight of the nine are in it; the ninth is `pub mod deep`,
/// which a `mod` declaration's `reportable: false` keeps out of
/// `unused_pub_item` entirely, so asserting it here would assert nothing.
/// What that module is for is the qualified path through it, and `thing` at
/// the end of that path is checked.
#[test]
fn a_binding_hides_the_item_it_shadows_and_only_the_item_it_shadows() {
    let analysis = analyze_fixture("scopes");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            // `let helper = 11;` — the motivating case of #8.
            ("src/lib.rs".to_string(), "helper"),
            // A function parameter binds for the whole body.
            ("src/lib.rs".to_string(), "width"),
            // The leaf of `let Pair(value) = ..` binds even though `Pair`
            // beside it is a use.
            ("src/lib.rs".to_string(), "value"),
            // A generic parameter binds in the type namespace, for its item.
            ("src/lib.rs".to_string(), "Marker"),
        ]
    );

    // Each of these is named exactly once, from a scope where a binding of the
    // same name is live. Any of them appearing above would be a live item
    // reported dead — the one outcome the analyzer must never produce.
    for name in [
        "Cfg",      // a type annotation beside `let mut Cfg = 12;`
        "seeded",   // `let seeded = seeded();` — the initializer comes first
        "fallback", // a `let ... else` block, where the binding does not exist
        "armed",    // the arm that does not bind the name
        "scoped",   // after the block that bound the name ended
        "Pair",     // the path of a tuple-struct pattern
        "LIMIT",    // a bare `const` pattern, which is a use and not a binding
        "thing",    // reached through `deep::thing()`, a qualified path
    ] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(name)),
            "`{name}` is referenced and must not be reported: {:?}",
            analysis.findings
        );
    }
}

// -- test-only items ---------------------------------------------------------
//
// The `testonly` fixture is a workspace of code that compiles and whose tests
// pass; every claim below is written down beside the item it is about.

/// Findings of the new kind, as `(file, name)` pairs in report order.
fn test_only(analysis: &Analysis) -> Vec<(String, &str)> {
    reported(analysis, FindingKind::TestOnlyItem)
}

/// The shipped default, and the reason the acceptance criterion for this phase
/// could stay "byte-identical": `off` produces no finding at all, so there is
/// nothing to change in the text, the JSON, the count or the exit code.
#[test]
fn the_test_only_kind_reports_nothing_until_a_project_asks_for_it() {
    let analysis = analyze_fixture("testonly");
    assert!(
        analysis.findings.is_empty(),
        "the fixture is quiet by default: {:?}",
        analysis.findings
    );
    assert!(!analysis.has_denied());
}

/// The claim, and its two halves. `only_tests` is reached — it is not an
/// unused-pub finding — and reached only from a `#[test]` function; `both` is
/// reached from `fn main` as well and must stay quiet.
#[test]
fn an_item_only_test_code_reaches_is_reported_and_one_main_reaches_is_not() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();

    assert!(names.contains(&"only_tests"), "{names:?}");
    assert!(
        !names.contains(&"both"),
        "`both` is reached from `fn main` too: {names:?}"
    );
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::UnusedPubItem),
        "a test-only item is alive, so nothing here is an unused-pub finding: {:?}",
        analysis.findings
    );
    assert_finding_message(
        &analysis,
        "only_tests",
        "is reached only from test code: make it `pub(crate)`, or move it behind `#[cfg(test)]`",
    );
}

/// The two halves of the split have to agree. `from_target` is reached from a
/// `harness = false` test target's `fn main` — no `#[test]` attribute anywhere
/// in that crate — and lands in the same kind as the inline `#[cfg(test)]`
/// case above.
#[test]
fn a_test_target_and_an_inline_test_module_are_classified_alike() {
    let analysis = analyze_configured("testonly", "warn.toml");
    assert_eq!(
        test_only(&analysis),
        vec![
            ("app/src/inline.rs".to_string(), "only_tests"),
            ("app/tests/support/mod.rs".to_string(), "from_target"),
            ("probe/src/hidden.rs".to_string(), "declared"),
            ("probe/src/hidden.rs".to_string(), "undeclared"),
        ],
    );
}

/// An opaque mention is a root in both walks, so it keeps its target out of
/// the kind. `mentioned` is named only by an `assert_eq!` in a `#[test]`
/// function — as test-only as an item gets — and is not reported.
#[test]
fn an_opaque_mention_keeps_an_item_out_of_the_test_only_kind() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(!names.contains(&"mentioned"), "{names:?}");
    // The comparison that gives the assertion above its teeth: the same shape
    // written without the macro *is* reported.
    assert!(names.contains(&"only_tests"), "{names:?}");
}

/// A library's public surface is a root in both walks: a consumer Deadwood
/// cannot see reaches it in a build with no tests at all.
#[test]
fn nothing_on_a_librarys_public_surface_is_ever_test_only() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(!names.contains(&"exported"), "{names:?}");
    // Nor is what a surface item reaches: `support` is `pub` in a private
    // module and only the tests reach `exported`, but a consumer we cannot see
    // reaches `exported` and `exported` reaches `support`.
    assert!(!names.contains(&"support"), "{names:?}");
    // Nor is anything a `pub use inner::*;` glob re-exports — `from_glob` is
    // `probe::facade::from_glob` to a consumer — nor anything in a `pub` module
    // the glob carries with it, one level further down.
    assert!(!names.contains(&"from_glob"), "{names:?}");
    assert!(!names.contains(&"deeper"), "{names:?}");
    // ...while the same crate's private module, which no consumer can name,
    // is judged like a binary's.
    assert!(names.contains(&"undeclared"), "{names:?}");
}

/// `[public-api]` is that same claim made outright rather than inferred, and
/// it keeps an item out of the kind for the same reason.
#[test]
fn a_declared_public_api_item_is_never_test_only() {
    let analysis = analyze_configured("testonly", "declared.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(!names.contains(&"declared"), "{names:?}");
    assert!(
        names.contains(&"undeclared"),
        "only the listed item is covered: {names:?}"
    );
}

/// Phase 3's guarantee, for the kind that had to bend its rule: `[severity]`,
/// `ignore`, the exit code and the JSON all cover a new kind by virtue of it
/// being a finding, with no plumbing of its own.
#[test]
fn the_test_only_kind_is_configurable_typed_and_forgiving_of_the_run() {
    let analysis = analyze_configured("testonly", "warn.toml");
    assert!(
        analysis
            .findings
            .iter()
            .all(|f| f.severity == Severity::Warn),
        "{:?}",
        analysis.findings
    );
    assert!(
        !analysis.has_denied(),
        "a `warn` kind cannot fail a run: {:?}",
        analysis.findings
    );

    let json: serde_json::Value =
        serde_json::from_str(&deadwood::report::render_json(&analysis).unwrap()).unwrap();
    assert_eq!(json["findings"][0]["kind"], "test_only_item");
    assert_eq!(json["findings"][0]["severity"], "warn");
    assert_eq!(json["findings"][0]["name"], "only_tests");

    let text = deadwood::report::render_text(&analysis);
    assert!(text.contains("Test-only public items (warn):\n"), "{text}");
}

/// And a baseline covers it too, which is the same guarantee from the other
/// end: the key is (kind, file, name), and this kind carries all three.
#[test]
fn a_test_only_finding_is_baselineable_like_any_other() {
    let dir = scratch_copy("testonly", "test-only");
    let config = dir.join("warn.toml");

    let (code, stdout) = run_binary(&dir, &["--config", config.to_str().unwrap()]);
    assert_eq!(code, Some(0), "warn findings do not fail a run: {stdout}");
    assert!(stdout.contains("4 finding(s)"), "{stdout}");

    let (code, stdout) = run_binary(
        &dir,
        &["--config", config.to_str().unwrap(), "--write-baseline"],
    );
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("Wrote 4 finding(s)"), "{stdout}");
    let recorded = std::fs::read_to_string(dir.join("deadwood-baseline.json")).unwrap();
    assert!(recorded.contains("\"test_only_item\""), "{recorded}");

    let (code, stdout) = run_binary(&dir, &["--config", config.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("No issues found."), "{stdout}");
    assert!(stdout.contains("4 finding(s) suppressed"), "{stdout}");
}
