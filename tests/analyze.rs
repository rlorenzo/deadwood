//! End-to-end tests: run the full analysis on the fixture packages under
//! `tests/fixtures/` and check the findings. Requires `cargo` on PATH (used
//! for `cargo metadata`), which is a given anywhere `cargo test` runs.

use std::path::{Path, PathBuf};

use serde_json::json;

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
    reported_under(analysis, kind, "")
}

/// The same, restricted to one package of a multi-member fixture, so two
/// phases sharing a workspace can each pin their own member exactly rather
/// than one of them owning the whole list.
fn reported_under<'a>(
    analysis: &'a Analysis,
    kind: FindingKind,
    prefix: &str,
) -> Vec<(String, &'a str)> {
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
        .filter(|(file, _)| file.starts_with(prefix))
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
    // `ReadmeDoctests` is pub and referenced by nothing *by design*: it is
    // `#[cfg(doctest)]`, compiled only for rustdoc, whose doctest collection
    // is its consumer — reporting it would tell the project to delete its
    // README's test coverage (#63).
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

/// A proc-macro crate's entry points are the compiler's to call: consumers
/// spell the derive, attribute, or macro the function registers — often under
/// a re-exported path in another workspace entirely — never the function, and
/// deleting it breaks every one of them ([#73]). The neighbour that registers
/// nothing still answers for itself.
///
/// [#73]: https://github.com/rlorenzo/deadwood/issues/73
#[test]
fn proc_macro_entry_points_are_never_unused() {
    let analysis = analyze_fixture("procmacro");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![("src/lib.rs".to_string(), "orphan")],
        "the compiler calls `derive_linked`, `host_fn` and `make_ident`; nothing calls `orphan`"
    );
}

/// An attribute macro the analyzer cannot expand may have written an export
/// beside the item it holds — bun's `#[bun_jsc::host_fn]` emits an
/// `extern "C"` shim, giving the item a caller outside Rust entirely — so
/// the item is never reported, and what its body names stays alive
/// ([#74]). The neighbours whose attributes rewrite nothing still answer
/// for themselves.
///
/// [#74]: https://github.com/rlorenzo/deadwood/issues/74
#[test]
fn an_item_under_an_unexpandable_attribute_macro_is_never_unused() {
    let analysis = analyze_fixture("attrexport");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![
            ("src/lib.rs".to_string(), "inert_orphan"),
            ("src/lib.rs".to_string(), "plain_orphan"),
        ],
        "`entry` may be exported by the macro and `helper`/`deeper` hang from its body; the orphans do not"
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
        reported_under(&analysis, FindingKind::UnusedPubItem, "facade/"),
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
        ],
    );

    // A binary has no surface for the glob in `main.rs` to reach. `tool`'s
    // whole list belongs to
    // `a_named_pub_use_of_a_module_in_a_binary_puts_nothing_on_a_surface_it_does_not_have`,
    // which pins the same claim for the other spelling.
    for name in ["caller", "from_glob"] {
        assert!(
            analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(name)),
            "`{name}` is in a binary, where no glob roots anything: {:?}",
            analysis.findings
        );
    }

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
        reported_under(&analysis, FindingKind::UnusedReexport, "facade/"),
        vec![("facade/src/imported.rs".to_string(), "Stale")],
    );
    assert_finding_message(
        &analysis,
        "from_import",
        "is referenced only from items that nothing reaches",
    );
}

/// `mod nook; pub use nook::sub;` puts `sub`'s items on a library's public
/// surface by a third route, beside the glob one
/// ([#28](https://github.com/rlorenzo/deadwood/issues/28)). This reports
/// *less*, so the assertions that matter most are again the ones that must
/// still be made — and the whole of `reexport`'s report is here, since no
/// registry crate produces a finding this rule can move.
#[test]
fn a_named_pub_use_of_a_module_puts_what_it_re_exports_on_the_public_surface() {
    let analysis = analyze_fixture("globs");

    assert_eq!(
        reported_under(&analysis, FindingKind::UnusedPubItem, "reexport/"),
        vec![
            // The dead referrer every other claim in the package is measured
            // against: nothing names it, and no consumer can name it either.
            ("reexport/src/dead.rs".to_string(), "unreached_referrer"),
            // `pub(crate) use guarded::locked;` re-exports nothing outward.
            (
                "reexport/src/guarded/locked.rs".to_string(),
                "still_reported"
            ),
            // A named `pub use` of an *item* carries that item and nothing
            // beside it. `Lifted` is quiet; its neighbour is not.
            ("reexport/src/item.rs".to_string(), "beside_the_lifted_one"),
            // The half rooting must not move: behind the re-export, so a
            // consumer can name it, and reported anyway because nothing does.
            ("reexport/src/nook/plain.rs".to_string(), "nothing_names_it"),
        ],
    );

    // The three items this phase silenced, one per spelling and one two hops
    // in. Each is something a consumer writes: `reexport::plain::…`,
    // `reexport::api::…`, `reexport::first::second::…`.
    for name in [
        "only_dead_names_it",
        "only_dead_names_it_too",
        "two_hops_from_the_root",
        "Lifted",
    ] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(name)),
            "`{name}` is nameable from outside: {:?}",
            analysis.findings
        );
    }

    assert_finding_message(
        &analysis,
        "nothing_names_it",
        "is never referenced by any resolved path",
    );
    assert_finding_message(
        &analysis,
        "still_reported",
        "is referenced only from items that nothing reaches",
    );
}

/// The half that must not move, stated on its own because it is most of the
/// behaviour: an item behind such a re-export that *nothing* names is reported
/// exactly as before. `unused_definitions` reports when `!(used && reached)`,
/// and the surface set feeds `reached` alone — so no surface rule can touch a
/// first-condition finding. That is why `syn`'s one finding, which sits inside
/// a module this phase newly reaches, does not move.
#[test]
fn an_item_behind_a_named_pub_use_of_a_module_that_nothing_names_is_still_reported() {
    let analysis = analyze_fixture("globs");

    assert!(
        reported_under(&analysis, FindingKind::UnusedPubItem, "reexport/")
            .contains(&("reexport/src/nook/plain.rs".to_string(), "nothing_names_it")),
        "rooting changes what an item's referrers prove, never whether it is \
         reported: {:?}",
        analysis.findings
    );
}

/// The conservatism half. `pub(crate) use` re-exports nothing outside the
/// crate, so it roots nothing however much it reads like the `pub use` above
/// it — the edge is the re-export's *visibility*, not merely that it names a
/// module.
#[test]
fn a_pub_crate_use_of_a_module_roots_nothing() {
    let analysis = analyze_fixture("globs");

    assert!(
        reported_under(&analysis, FindingKind::UnusedPubItem, "reexport/").contains(&(
            "reexport/src/guarded/locked.rs".to_string(),
            "still_reported"
        )),
        "no consumer can go through a crate-visible re-export: {:?}",
        analysis.findings
    );
}

/// The route that needed no fixing, and the one a wider rule would break. A
/// named `pub use` of an *item* is an edge to that item; reading it as a
/// surface fact would root everything in the module holding it.
#[test]
fn a_named_pub_use_of_an_item_carries_that_item_and_not_the_module_holding_it() {
    let analysis = analyze_fixture("globs");

    assert!(
        reported_under(&analysis, FindingKind::UnusedPubItem, "reexport/")
            .contains(&("reexport/src/item.rs".to_string(), "beside_the_lifted_one")),
        "`Lifted` is reached through the re-export and its neighbour is not: {:?}",
        analysis.findings
    );
}

/// A binary has no public surface for either spelling to reach: no module of a
/// non-library crate seeds the closure, so nothing in one is ever put on it.
/// The whole of `tool`'s report is here, both forms together.
#[test]
fn a_named_pub_use_of_a_module_in_a_binary_puts_nothing_on_a_surface_it_does_not_have() {
    let analysis = analyze_fixture("globs");

    assert_eq!(
        reported_under(&analysis, FindingKind::UnusedPubItem, "tool/"),
        vec![
            ("tool/src/dead_end.rs".to_string(), "caller"),
            ("tool/src/hidden.rs".to_string(), "from_glob"),
            (
                "tool/src/tucked/inner.rs".to_string(),
                "from_named_reexport"
            ),
        ],
    );
    assert_eq!(
        reported_under(&analysis, FindingKind::UnusedReexport, "tool/"),
        vec![("tool/src/main.rs".to_string(), "inner")],
        "the crate root of a binary is not on any surface, so the re-export \
         itself has nothing to excuse it either",
    );
}

/// The rule is a closure and not a pass: `first` is on the surface only
/// because the crate root re-exports it, and the `pub use` written *inside*
/// `first` is then an edge from a module the new edge itself put in the set.
#[test]
fn the_surface_closure_follows_a_named_pub_use_two_hops_from_the_crate_root() {
    let analysis = analyze_fixture("globs");

    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("two_hops_from_the_root")),
        "`reexport::first::second::two_hops_from_the_root` needs both hops: {:?}",
        analysis.findings
    );
}

/// The second consumer of the same set. `is_worth_reporting` asks the surface
/// question about a `pub use` too, so widening the set for the root set widens
/// it here — a `pub use` in a module a named `pub use` reaches is doing its job
/// by existing. Phase 11 existed to stop these two from drifting apart, and a
/// phase that widened one and not the other would have reintroduced the drift.
#[test]
fn a_pub_use_inside_a_module_a_named_pub_use_reaches_is_not_reported() {
    let analysis = analyze_fixture("globs");

    assert_eq!(
        reported_under(&analysis, FindingKind::UnusedReexport, "reexport/"),
        Vec::new(),
        "`Alias` sits in a module the new edge reaches, and `second` in one it \
         reaches two hops in: {:?}",
        analysis.findings
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
            // Named nowhere, and `lonely_derived` — mentioned, but not a
            // proc-macro companion (`_derived` is no companion suffix) —
            // spares it nothing (#64). The pair beside it pins the opposite:
            // `widget` is absent from this list because `widget_derive` is
            // declared and mentioned, and a derive's expansion names its
            // base crate.
            ("Cargo.toml".to_string(), "lonely"),
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
        // body of a `#[cfg(test)] mod` names, three named only from a function
        // a test attribute confines, one named only from a function an
        // attribute macro owns *in a test target* (where the macro changes
        // nothing), and a build-dependency the build script never touches.
        // Then the direction phase 21 added: five `[dev-dependencies]` entries
        // the library itself names — three of them from under attributes
        // (built-in, tool, derive helper) that are not macros and so leave
        // their item attributed as written.
        vec![
            "attr_macro_test_target_crate",
            "bench_fn_crate",
            "builtin_attr_dev_crate",
            // Named only from a `#[cfg(doctest)] fn`: the one build that
            // compiles the mention links `[dev-dependencies]`, so the normal
            // entry costs every consumer a build for nothing (#63).
            "doctest_only_normal_crate",
            "example_only_crate",
            "helper_attr_dev_crate",
            "library_and_test_dev_crate",
            "library_named_dev_crate",
            "nested_test_fn_crate",
            "outline_test_crate",
            "stale_build_crate",
            // The dev copy of a doubled crate that no dev code names: a stale
            // duplicate, reported on its own absent evidence.
            "stale_dev_copy_crate",
            "test_fn_crate",
            "test_only_crate",
            "tool_attr_dev_crate",
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
        // A dev-dependency named only from a bare `#[test] fn` at module
        // scope, which is the shape that made `clap_builder`'s
        // `static_assertions` look like library code.
        "test_fn_dev_crate",
        // `#[should_panic]` on its own leaves the function in the library
        // build, and an attribute macro Deadwood cannot expand says nothing
        // about where its function is compiled: its item is macro input, so
        // the mention is opaque and the entry is not judged.
        "should_panic_crate",
        "proc_macro_test_crate",
        // The macro's own crate: named by plain library code, correctly
        // declared as a dependency.
        "attr_macro_host_crate",
        // The invented finding #49 filed: a dev-dependency named only from a
        // `#[attr_macro_host_crate::test] fn` in the library, a manifest that
        // compiles because the macro expands to the built-in `#[test]`.
        "attr_macro_dev_crate",
        // The same through a single-segment attribute macro brought in by
        // `use`, through the `#[core::prelude::v1::test]` spelling the
        // built-in attribute expands to, and on an associated fn inside an
        // `impl` block, where a `#[test]` could never confine but a macro
        // still owns its item.
        "single_segment_attr_dev_crate",
        "core_prelude_test_dev_crate",
        "attr_macro_impl_dev_crate",
        // Declared in both tables with dev code naming it: the library
        // mentions are the `[dependencies]` copy's evidence, the dev mention
        // is the dev copy's own, and both copies are where they belong.
        "doubled_features_crate",
        // Declared in both tables with *nothing* dev naming it, but the dev
        // copy enables what the `[dependencies]` copy does not — an extra
        // feature for one, default features for the other. The copy is load
        // bearing without being named (#61), so neither is a finding.
        "loadbearing_dev_copy_crate",
        "defaults_off_crate",
        // A `[build-dependencies]` entry named only from a `#[test] fn` inside
        // `build.rs`. A build script has no test harness, so its test
        // functions are build-script code like the rest of the file.
        "build_test_crate",
        // A dev-dependency named only from inside a `macro_rules!` body. The
        // new claim needs a runtime mention, and a mention Deadwood cannot see
        // through is not one.
        "opaque_dev_crate",
        // A dev-dependency named only by `build.rs`. The build script cannot
        // link it either, but that says nothing about which of the other two
        // tables it belongs in, so no claim is made.
        "build_only_dev_crate",
        // Named by library code *and* by a doc comment. The opaque mention
        // stops the entry being judged, even though the library mention alone
        // would place it: the guard costs a finding rather than inventing one.
        "doc_and_library_dev_crate",
        // The regex shape (#63): a dev-dependency named only from a
        // `#[cfg(doctest)]` item-position macro invocation. rustdoc links the
        // dev-dependencies when it collects doctests, so the manifest
        // compiles and the entry is exactly where it belongs.
        "cfg_doctest_dev_crate",
        // The tokio shape (#63): a dev-dependency named only from the text of
        // a `#[doc = "..."]` attribute inside a macro body. Doc text is
        // documentation wherever it sits — an opaque mention, alive and
        // unjudged.
        "doc_literal_dev_crate",
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

/// The claim phase 5 refused, made in phase 21: a `[dev-dependencies]` entry
/// the library itself names belongs in `[dependencies]`.
///
/// The manifest does not compile — `cargo build` cannot see a dev-dependency
/// from the library — so this is a real defect rather than a style question.
/// What kept it unmade was not the reasoning but the evidence: while a
/// mis-attribution of ours was the likelier explanation the claim would have
/// invented findings, and #14 closed the first of those, #44 the second.
#[test]
fn a_dev_dependency_the_library_names_is_reported_as_belonging_in_dependencies() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert!(
        reported_names.contains(&"library_named_dev_crate"),
        "named by `pub fn build`, which is a build that cannot link it: {:?}",
        analysis.findings
    );
    // The message has to say which way the entry moves, or a reader cannot
    // tell it from the claim that moves an entry the other way.
    let message = analysis
        .findings
        .iter()
        .find(|finding| finding.name.as_deref() == Some("library_named_dev_crate"))
        .map(|finding| finding.message.clone())
        .unwrap_or_default();
    assert!(
        message.contains("belongs in `[dependencies]` rather than `[dev-dependencies]`"),
        "the direction has to be in the message: {message}"
    );
    // And the evidence clause has to describe *this* claim. Reusing the other
    // direction's wording would tell a reader the tests name it, which is the
    // opposite of what was found and the opposite of what to do about it.
    assert!(
        message.contains("is referenced by the library"),
        "the evidence clause belongs to this direction, not the other: {message}"
    );
    assert!(
        !message.contains("only by the test"),
        "that is the other direction's evidence: {message}"
    );
}

/// A doubled dev copy is claimed as the duplicate it is, not as a move.
///
/// The move wording — "referenced by the library, …, which cannot link a
/// dev-dependency" — is false on both counts for a doubled crate: the
/// `[dependencies]` copy links it and the manifest compiles. What is true of
/// `stale_dev_copy_crate` is that the copy adds nothing, and the message says
/// exactly that ([#61](https://github.com/rlorenzo/deadwood/issues/61)).
#[test]
fn a_stale_dev_copy_is_worded_as_a_duplicate_rather_than_a_move() {
    let analysis = analyze_fixture("depkinds");
    let message = analysis
        .findings
        .iter()
        .find(|finding| finding.name.as_deref() == Some("stale_dev_copy_crate"))
        .map(|finding| finding.message.clone())
        .unwrap_or_default();
    assert!(
        message.contains("duplicates the `[dependencies]` entry"),
        "the claim is the duplication: {message}"
    );
    assert!(
        message.contains("stale"),
        "and the advice is removal, not a move: {message}"
    );
    assert!(
        !message.contains("cannot link"),
        "the move wording is false here — the `[dependencies]` copy links it: {message}"
    );
}

/// The asymmetry between the two directions, which is not obvious and is the
/// thing most likely to be flattened into a bug.
///
/// An entry moves *down* only when every mention is dev code, because one
/// library mention justifies it where it is. It moves *up* on a single runtime
/// mention, because the library cannot link a dev-dependency at all — so test
/// code naming it too changes nothing.
#[test]
fn a_dev_dependency_both_the_library_and_the_tests_name_still_belongs_in_dependencies() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert!(
        reported_names.contains(&"library_and_test_dev_crate"),
        "`tests/it.rs` naming it as well does not make the library build work: {:?}",
        analysis.findings
    );
}

/// A crate alias binds for the crate that declares it, and mentions of the
/// alias belong to the crate it renames — not to a manifest entry that happens
/// to share its spelling.
///
/// `serde_json` is the live instance: `extern crate serde_core as serde;` beside
/// a `serde` dev-dependency, so every `serde::` in its library means
/// `serde_core`. Without this the library appears to name its own
/// dev-dependency, which phase 21's claim turns into a finding invented against
/// a manifest that compiles
/// ([#48](https://github.com/rlorenzo/deadwood/issues/48)).
#[test]
fn a_renamed_extern_crate_does_not_charge_its_mentions_to_the_entry_it_shadows() {
    let analysis = analyze_fixture("depkinds");
    let misplaced: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert!(
        !misplaced.contains(&"aliased_crate"),
        "`aliased_crate::` in the library means `renamed_core_crate`, so the \
         dev-dependency of that name is not named by library code: {:?}",
        analysis.findings
    );
    assert!(
        !misplaced.contains(&"renamed_core_crate"),
        "and the crate it actually names is a normal dependency the library \
         uses, which is where it belongs: {:?}",
        analysis.findings
    );
    // `use real as alias;` binds a crate the same way and is the edition-2018
    // spelling of the same rename, so it gets the same pair of answers.
    assert!(
        !misplaced.contains(&"use_aliased_crate"),
        "the `use` spelling of the rename binds a crate too: {:?}",
        analysis.findings
    );
    assert!(
        !misplaced.contains(&"use_renamed_crate"),
        "and the crate it names is where it belongs: {:?}",
        analysis.findings
    );
}

/// A rename binds where it is written, and an alias inside a nested module
/// does not reach the code around it.
///
/// `depkinds` writes `mod nested { use nested_renamed_crate as
/// nested_alias_crate; }` and then names `nested_alias_crate` at the crate
/// root, where that alias does not apply. Folding a nested alias over the
/// whole file moves the crate-root mention onto the wrong crate and reports
/// the `nested_alias_crate` dependency as unused — a finding invented against
/// code that compiles.
#[test]
fn an_alias_inside_a_module_does_not_reach_the_code_around_it() {
    let analysis = analyze_fixture("depkinds");
    let unused: Vec<&str> = reported(&analysis, FindingKind::UnusedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    for entry in [
        // The `use` spelling, aliased inside `mod nested`.
        "nested_alias_crate",
        "nested_renamed_crate",
        // The `extern crate` spelling, aliased inside `mod nested_extern`.
        "nested_extern_alias",
        "nested_extern_renamed",
        // And the cross-file case: a rename at the crate root binds in the
        // root module only, so `src/crossfile.rs` sees the crate itself.
        "crossfile_alias_crate",
        "crossfile_renamed_crate",
        // The mirror of it: an `extern crate` rename at the top of a *module
        // file* is an ordinary item of that module, not an extern-prelude
        // entry, so it does not reach the crate root either.
        "modfile_alias_crate",
        "modfile_renamed_crate",
    ] {
        assert!(
            !unused.contains(&entry),
            "`{entry}` is named — the rename does not reach where it is named: {:?}",
            analysis.findings
        );
    }
}

/// Where the rename rule stops: `use crate_name::Item as Alias;` renames an
/// item, not a crate.
///
/// The head of that path is still the crate, and `Alias` is a type in the
/// module that wrote it. Folding on it would move a dependency's mentions onto
/// whatever crate the path happened to start with — `depkinds` writes
/// `use shared_crate::Thing as cfg_test_crate;` so that mistake reports the
/// `cfg_test_crate` dev-dependency as unused.
#[test]
fn a_renamed_item_is_not_a_renamed_crate() {
    let analysis = analyze_fixture("depkinds");
    let unused: Vec<&str> = reported(&analysis, FindingKind::UnusedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert!(
        !unused.contains(&"cfg_test_crate"),
        "the `#[cfg(test)]` module names it, and the item rename above changes \
         nothing about that: {:?}",
        analysis.findings
    );
}

/// The other half, and the one a package-wide fold gets wrong.
///
/// `tests/it.rs` is a separate crate that links the dev-dependencies directly;
/// `src/lib.rs`'s rename does not reach it, so `aliased_crate::` written there
/// is the dev-dependency for real. Folding the alias across the whole package
/// instead of per target reports that entry as unused — which is what a first
/// cut of #48 did, and what `serde_json` caught.
#[test]
fn a_crate_alias_does_not_reach_a_separate_test_crate() {
    let analysis = analyze_fixture("depkinds");
    let unused: Vec<&str> = reported(&analysis, FindingKind::UnusedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    assert!(
        !unused.contains(&"aliased_crate"),
        "`tests/it.rs` names it, and the lib's rename does not apply there: {:?}",
        analysis.findings
    );
    assert!(
        !unused.contains(&"renamed_core_crate"),
        "the `extern crate` declaration is itself a mention of it: {:?}",
        analysis.findings
    );
    for entry in ["use_aliased_crate", "use_renamed_crate"] {
        assert!(
            !unused.contains(&entry),
            "`{entry}` is named, through the `use` spelling of the rename: {:?}",
            analysis.findings
        );
    }
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

/// `#[test]` confines a function to a test build as completely as
/// `#[cfg(test)]` does, and it is not a `cfg` — so the gate machinery could not
/// see it, and every mention inside a bare `#[test] fn` read as library code.
///
/// The finding that cost: a `[dependencies]` entry named only from one is a
/// `[dev-dependencies]` entry with the wrong table written above it, and until
/// now the check said nothing about it. Verified against rustc rather than
/// assumed — `rustc --crate-type=lib` compiles a bare `#[test] fn` naming a
/// crate that does not exist, and `rustc --test` fails on it with `E0433`.
#[test]
fn a_dependency_named_only_from_a_test_function_belongs_in_dev_dependencies() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();

    for entry in [
        // A bare `#[test] fn` at module scope in `src/lib.rs`.
        "test_fn_crate",
        // `#[bench]`, which confines a function identically.
        "bench_fn_crate",
    ] {
        assert!(
            reported_names.contains(&entry),
            "`{entry}` is named only from a function no non-test build compiles: {:?}",
            analysis.findings
        );
    }
}

/// The direction the same answer has to move in for a `[dev-dependencies]`
/// entry, and the reason this had to land before any claim about one: the
/// manifest is correct, and the mention that made it look broken is test code.
///
/// Measured on the corpus, this is `clap_builder`'s `static_assertions` (four
/// mentions, three of them in bare `#[test] fn`s) and `winnow`'s
/// `term-transcript` (one, in a bare `#[test] fn` carrying `cfg` gates that
/// hold outside a test build). Both were candidates for that claim before this;
/// neither is now.
#[test]
fn a_dev_dependency_named_only_from_a_test_function_is_left_alone() {
    let analysis = analyze_fixture("depkinds");
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("test_fn_dev_crate")),
        "the entry is declared in the only table its mention can see: {:?}",
        analysis.findings
    );
}

/// The boundary. `#[should_panic]` accompanies a test attribute and confines
/// nothing by itself — rustc resolves the body of a `#[should_panic] fn` under
/// `--crate-type=lib` — and an attribute macro Deadwood cannot expand is a
/// guess in both directions: `#[tokio::test]` really does confine, an attribute
/// merely *named* `test` need not, and nothing before expansion tells them
/// apart. So only the built-in, single-segment `test` and `bench` count as
/// confinement — and the macro case is no longer read as library code either:
/// its item is macro input, so the mention is opaque and the entry is not
/// judged. Both entries stay unreported, one placed and one unjudged.
///
/// [`crate::resolve`] matches the same two names the opposite way — on the last
/// path segment, so `#[tokio::test]` is the test entry point it is — because
/// there an over-eager match can only keep an item alive. Here it would move a
/// mention out of the library and invent a finding against a manifest that
/// compiles.
#[test]
fn an_attribute_deadwood_cannot_expand_does_not_confine_a_dependency_to_the_tests() {
    let analysis = analyze_fixture("depkinds");
    for entry in ["should_panic_crate", "proc_macro_test_crate"] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is named by code the library compiles: {:?}",
            analysis.findings
        );
    }
}

/// The finding [#49](https://github.com/rlorenzo/deadwood/issues/49) filed as
/// invented, in every spelling the fixture pins. A `#[tokio::test]`-shaped
/// macro expands to the built-in `#[test]`, so a `[dev-dependencies]` entry
/// named only from the function it owns is correctly declared — and phase 21's
/// claim, reading that mention as library code, reported it as belonging in
/// `[dependencies]` against a manifest that compiles. The item is macro input
/// now: known used, unknown where, no claim in either direction.
///
/// The spellings: a multi-segment attribute path (`#[attr_macro_host_crate::
/// test]`), a single-segment attribute brought into scope by `use` (no
/// built-in of that name, no `#[derive]` to be a helper of), the
/// `#[core::prelude::v1::test]` path the built-in attribute expands to, and a
/// macro on an associated fn — where `#[test]` could never confine
/// ([`Site::Other`]) but a macro still owns its item. The macro's own crate is
/// also here: named by plain library code, placed by it, and not judged
/// through its opaque attribute-path mentions.
#[test]
fn an_entry_named_only_under_an_attribute_macro_is_not_judged() {
    let analysis = analyze_fixture("depkinds");
    for entry in [
        "attr_macro_dev_crate",
        "single_segment_attr_dev_crate",
        "core_prelude_test_dev_crate",
        "attr_macro_impl_dev_crate",
        "attr_macro_host_crate",
    ] {
        assert!(
            !analysis
                .findings
                .iter()
                .any(|f| f.name.as_deref() == Some(entry)),
            "`{entry}` is under (or is) an attribute macro, which owns its item: {:?}",
            analysis.findings
        );
    }
}

/// What must *not* be swept into that opacity: the attribute kinds that
/// rewrite nothing. A built-in attribute (`#[inline]`), a tool attribute
/// (`#[rustfmt::skip]` — the corpus's most common spelling), and a derive
/// helper (`#[fake_helper(..)]` beside `#[derive(FakeSerialize)]`) all leave
/// their item attributed as written, so a `[dev-dependencies]` entry each one
/// names from library code is still the finding phase 21 made it.
#[test]
fn an_inert_attribute_leaves_its_item_attributed_as_written() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    for entry in [
        "builtin_attr_dev_crate",
        "tool_attr_dev_crate",
        "helper_attr_dev_crate",
    ] {
        assert!(
            reported_names.contains(&entry),
            "`{entry}` is named by library code under an attribute that is not a macro: {:?}",
            analysis.findings
        );
    }
}

/// The scope of the shift: runtime code only. Expansion happens inside one
/// crate, so whatever a macro leaves of an item in a test target compiles into
/// that same dev build — the attribution written there holds whatever the
/// macro does, and a `[dependencies]` entry named only from a
/// `#[attr_macro_host_crate::test] fn` in `tests/it.rs` is reported exactly as
/// `test_only_crate` is.
#[test]
fn an_attribute_macro_in_a_dev_target_changes_nothing() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert!(
        reported_names.contains(&"attr_macro_test_target_crate"),
        "{:?}",
        analysis.findings
    );
}

/// The doubled manifest [#55](https://github.com/rlorenzo/deadwood/issues/55)
/// filed, in both of its spellings. `doubled_features_crate` is declared in
/// both tables and named by both kinds of code: the library mentions justify
/// the `[dependencies]` copy, the `tests/it.rs` mention justifies the dev
/// copy, and reporting either — the dev copy was reported as belonging in
/// `[dependencies]` — indicts a manifest that compiles.
/// `stale_dev_copy_crate` is doubled with *no* dev mention anywhere: its dev
/// copy duplicates an entry the library already justifies, and that claim
/// rests on the dev code's silence — this entry's own evidence — so it stays
/// a finding, exactly as `stale_build_crate`'s build copy does.
#[test]
fn a_doubled_crate_moves_only_on_the_evidence_of_its_own_entry() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert!(
        !reported_names.contains(&"doubled_features_crate"),
        "both copies carry their own table's mentions: {:?}",
        analysis.findings
    );
    assert!(
        reported_names.contains(&"stale_dev_copy_crate"),
        "a dev copy nothing dev names is a stale duplicate: {:?}",
        analysis.findings
    );
}

/// The nested case, which the module already answered: a `#[cfg(test)] mod
/// tests` moves its whole subtree, so a bare `#[test] fn` inside one adds
/// nothing and must not be a second, separate answer about the same mention.
/// `nested_test_fn_crate` is reported here exactly as it was before `#[test]`
/// counted for anything.
#[test]
fn a_test_function_inside_a_cfg_test_module_is_attributed_once() {
    let analysis = analyze_fixture("depkinds");
    let reported_names: Vec<&str> = reported(&analysis, FindingKind::MisplacedDependency)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert!(
        reported_names.contains(&"nested_test_fn_crate"),
        "{:?}",
        analysis.findings
    );
    assert!(
        !reported_names.contains(&"cfg_test_crate"),
        "and the dev-dependency the same module names is still correctly placed: {:?}",
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
    scratch_unconfigured("baseline", test)
}

/// The same, for any fixture whose `*.toml` files each name a baseline: the
/// copies come along with the tree and would only confuse a reader of a run
/// that uses the default location.
fn scratch_unconfigured(fixture: &str, test: &str) -> PathBuf {
    let dir = scratch_copy(fixture, test);
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

    // `all-baseline.json` was written by `--write-baseline` before the key
    // carried a module, and is checked in exactly as that run left it: a file
    // from an older Deadwood, read by this one with no edit. That it still
    // suppresses all six findings is the upgrade promise, and it is only a test
    // of that while the file genuinely predates the field.
    let recorded = std::fs::read_to_string(fixtures().join("baseline/all-baseline.json")).unwrap();
    assert!(!recorded.contains("module"), "{recorded}");

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

/// The upgrade path, and the whole compatibility claim of the module field, in
/// a file checked in before that field existed: `collision-baseline.json` names
/// no module, so it still covers both twins and reports nothing stale. An entry
/// that says nothing about modules has not said the modules differ.
#[test]
fn an_entry_with_no_module_still_covers_every_finding_that_shares_its_key() {
    let recorded =
        std::fs::read_to_string(fixtures().join("baseline/collision-baseline.json")).unwrap();
    assert!(
        !recorded.contains("module"),
        "this case is only a test of the fallback while the file predates the field:\n{recorded}"
    );

    let analysis = analyze_configured("baseline", "collision.toml");
    assert!(
        reported(&analysis, FindingKind::UnusedPubItem)
            .iter()
            .all(|(_, name)| *name != "twin"),
        "both twins are suppressed by the one entry: {:?}",
        analysis.findings
    );
    assert_eq!(suppressed(&analysis), 2);
    assert!(
        stale_keys(&analysis).is_empty(),
        "and the entry is not stale either: {:?}",
        stale_keys(&analysis)
    );
}

/// The phase: the same two twins, recorded with the module they are written in.
/// One entry now covers one twin, and the other twin — a finding that could
/// never reach the report before — is reported at its own line.
///
/// Every line in `modules-baseline.json` is wrong, so this also pins that the
/// module did not reintroduce the drift the line was kept out of the key to
/// avoid; and one entry names `crate::gamma`, which nothing occurs in, so an
/// implementation that recorded the module without comparing it would suppress
/// both twins and report nothing stale.
#[test]
fn an_entry_naming_a_module_leaves_its_same_named_neighbour_reported() {
    let analysis = analyze_configured("baseline", "modules.toml");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![("src/lib.rs".to_string(), "twin")],
        "the unrecorded twin is the only finding: {:?}",
        analysis.findings
    );
    assert_eq!(
        analysis.findings[0].line,
        Some(15),
        "and it points at `beta::twin`, not at the baselined line"
    );
    assert_eq!(
        analysis.findings[0].module.as_deref(),
        Some("crate::beta"),
        "{:?}",
        analysis.findings
    );
    assert!(analysis.has_denied(), "so it fails the run");
    assert_eq!(suppressed(&analysis), 5, "everything else stayed quiet");

    assert_eq!(
        stale_keys(&analysis),
        vec!["src/lib.rs: unused_pub_item `twin` in `crate::gamma`".to_string()],
        "and the module nothing occurs in is named in the stale list"
    );
}

// -- a moved file ----------------------------------------------------------
//
// The `moved` fixture is a two-member workspace producing six findings, and
// every `*.toml` beside it records them at files they are no longer in. Two
// members, because the question is whether a `crate`-relative module path can
// stand in for the file, and one package cannot ask it.

/// The unconfigured run every case below is measured against.
#[test]
fn the_moved_fixture_reports_one_finding_of_each_shape_it_needs() {
    let analysis = analyze_fixture("moved");
    assert_eq!(
        all_reported(&analysis),
        vec![
            (
                FindingKind::UnusedDependency,
                "alpha/Cargo.toml".to_string(),
                "unused_crate".to_string()
            ),
            (
                FindingKind::DeadFile,
                "alpha/src/attic/dropped.rs".to_string(),
                String::new()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/bin/one.rs".to_string(),
                "shared".to_string()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/bin/two.rs".to_string(),
                "shared".to_string()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/legacy/mod.rs".to_string(),
                "gone".to_string()
            ),
            (
                FindingKind::UnusedPubItem,
                "beta/src/lib.rs".to_string(),
                "migrated".to_string()
            ),
        ]
    );
    assert!(
        analysis.baseline.is_none(),
        "no `baseline` key and no file at the default location is no baseline"
    );
}

/// The headline case of #17: `git mv` changes no item, so a baseline written
/// before the move still covers everything and the run is silent. Two shapes of
/// move at once — a file module that became a directory module, and an
/// out-of-line module that became an inline one — and both leave the item where
/// it was in its crate's module tree.
#[test]
fn a_finding_whose_file_moved_is_still_baselined() {
    let analysis = analyze_configured("moved", "moved.toml");
    assert!(
        analysis.findings.is_empty(),
        "a pure rename must not fail the run: {:?}",
        all_reported(&analysis)
    );
    assert!(!analysis.has_denied());
    assert_eq!(suppressed(&analysis), 6);
    assert!(
        stale_keys(&analysis).is_empty(),
        "and the entries that recorded them are not stale either: {:?}",
        stale_keys(&analysis)
    );
}

/// The case the cheap fix gets wrong. `one.rs` and `two.rs` each define a
/// `pub fn shared` at `crate` in one package, so they differ in nothing but the
/// file; baselining one must not cover the other. This is the test that goes
/// red the moment the matcher stops consulting the file first.
#[test]
fn two_items_sharing_a_name_and_a_module_in_two_files_are_still_two_findings() {
    let analysis = analyze_configured("moved", "neighbour.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::UnusedPubItem,
            "alpha/src/bin/two.rs".to_string(),
            "shared".to_string()
        )],
        "the neighbour of a baselined item is news"
    );
    assert!(analysis.has_denied(), "so it fails the run");
    assert_eq!(suppressed(&analysis), 5);
    assert!(
        stale_keys(&analysis).is_empty(),
        "and nothing went stale: {:?}",
        stale_keys(&analysis)
    );
}

/// One entry and two candidates is not a move, it is two readings of the same
/// evidence. The run reports both findings and names the entry stale —
/// declining is what keeps the failure mode noise rather than silence.
#[test]
fn an_entry_with_two_candidates_relocates_to_neither() {
    let analysis = analyze_configured("moved", "ambiguous.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![
            (
                FindingKind::UnusedPubItem,
                "alpha/src/bin/one.rs".to_string(),
                "shared".to_string()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/bin/two.rs".to_string(),
                "shared".to_string()
            ),
        ]
    );
    assert_eq!(
        stale_keys(&analysis),
        vec!["alpha/src/bin/three.rs: unused_pub_item `shared` in `crate`".to_string()]
    );
}

/// The same refusal the other way round: two entries competing for one finding
/// leaves the finding reported and both entries stale.
#[test]
fn two_entries_competing_for_one_finding_relocate_to_neither() {
    let analysis = analyze_configured("moved", "crowded.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::UnusedPubItem,
            "alpha/src/legacy/mod.rs".to_string(),
            "gone".to_string()
        )]
    );
    assert_eq!(
        stale_keys(&analysis),
        vec![
            "alpha/src/legacy.rs: unused_pub_item `gone` in `crate::legacy`".to_string(),
            "alpha/src/attic/legacy.rs: unused_pub_item `gone` in `crate::legacy`".to_string(),
        ]
    );
}

/// A module path is `crate`-relative and says nothing about which crate. Every
/// other signal agrees here — kind, name, module, one candidate on each side —
/// and the entry still does not travel, because the packages differ.
#[test]
fn a_moved_finding_is_not_matched_across_two_packages() {
    let analysis = analyze_configured("moved", "crosspackage.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::UnusedPubItem,
            "beta/src/lib.rs".to_string(),
            "migrated".to_string()
        )],
        "`beta`'s `crate::legacy::migrated` is not `alpha`'s"
    );
    assert_eq!(
        stale_keys(&analysis),
        vec!["alpha/src/migrated.rs: unused_pub_item `migrated` in `crate::legacy`".to_string()]
    );
}

/// The boundary of the phase, pinned rather than assumed. A `dead_file` carries
/// no name and no module, and a dependency entry names a crate in a manifest —
/// neither has an item identity a move could preserve, so both behave exactly as
/// they did before the second pass existed.
#[test]
fn a_finding_with_no_module_is_never_relocated() {
    let analysis = analyze_configured("moved", "unmoved.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![
            (
                FindingKind::UnusedDependency,
                "alpha/Cargo.toml".to_string(),
                "unused_crate".to_string()
            ),
            (
                FindingKind::DeadFile,
                "alpha/src/attic/dropped.rs".to_string(),
                String::new()
            ),
        ],
        "both are reported again at their new locations"
    );
    assert_eq!(
        stale_keys(&analysis),
        vec![
            "beta/Cargo.toml: unused_dependency `unused_crate`".to_string(),
            "alpha/src/dropped.rs: dead_file".to_string(),
        ],
        "and both entries are stale, which is the whole of today's behaviour"
    );
}

/// Pruning removes exactly the entries the run named stale, and leaves a
/// relocated one in place — with the path it was written with, exactly as a
/// matched entry keeps its drifted line. `--write-baseline` is what re-records
/// either.
#[test]
fn pruning_keeps_a_relocated_entry_with_the_path_it_was_written_with() {
    let dir = scratch_copy("moved", "prune-relocated");
    let config = dir.join("moved.toml");
    let path = dir.join("moved-baseline.json");

    // One entry nothing can match, added on purpose: without it a
    // `--prune-baseline` that did nothing at all would pass this test, and the
    // claim is that pruning tells a relocated entry from a stale one — not that
    // it leaves the file alone.
    let mut source: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    source["findings"].as_array_mut().unwrap().push(json!({
        "kind": "dead_file",
        "file": "alpha/src/never-existed.rs"
    }));
    std::fs::write(&path, serde_json::to_string_pretty(&source).unwrap()).unwrap();

    let (code, out) = run_binary(
        dir.as_path(),
        &["--config", config.to_str().unwrap(), "--prune-baseline"],
    );
    assert_eq!(code, Some(0), "{out}");

    let file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let entries = file["findings"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        6,
        "the stale entry went and the other six stayed: {entries:#?}"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| entry["file"] == "alpha/src/never-existed.rs"),
        "the entry nothing matched is gone: {entries:#?}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["file"] == "alpha/src/legacy.rs" && entry["line"] == 902),
        "and the relocated entry is untouched, path and line alike: {entries:#?}"
    );
}

// -- what a move does not survive, and why that is the answer ---------------
//
// Phase 17 measured #32 and closed it: the four kinds with no item identity
// keep behaving exactly as they do above, and the boundary is pinned here
// rather than only argued in `docs/HISTORY.md`. Each test below is a case a rule
// that closed #32 would have had to get right, and each one is a case the rule
// available gets wrong.

/// The second event #32 describes, and the one the item kinds do not survive
/// either. A whole package directory moved, so every entry recording one of its
/// findings — the manifest entry and the dead file, which never had an item
/// identity, and the three `unused_pub_item`s, which do — names a path inside
/// no package of this workspace. The pass resolves a recorded path to a package
/// by containment, gets nothing, and declines for all five alike.
#[test]
fn a_moved_package_directory_relocates_none_of_its_findings() {
    let analysis = analyze_configured("moved", "movedpackage.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![
            (
                FindingKind::UnusedDependency,
                "alpha/Cargo.toml".to_string(),
                "unused_crate".to_string()
            ),
            (
                FindingKind::DeadFile,
                "alpha/src/attic/dropped.rs".to_string(),
                String::new()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/bin/one.rs".to_string(),
                "shared".to_string()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/bin/two.rs".to_string(),
                "shared".to_string()
            ),
            (
                FindingKind::UnusedPubItem,
                "alpha/src/legacy/mod.rs".to_string(),
                "gone".to_string()
            ),
        ],
        "every finding of the moved package is reported again"
    );
    assert_eq!(
        stale_keys(&analysis),
        vec![
            "vendor/alpha/Cargo.toml: unused_dependency `unused_crate`".to_string(),
            "vendor/alpha/src/attic/dropped.rs: dead_file".to_string(),
            "vendor/alpha/src/bin/one.rs: unused_pub_item `shared` in `crate` (value namespace)"
                .to_string(),
            "vendor/alpha/src/bin/two.rs: unused_pub_item `shared` in `crate` (value namespace)"
                .to_string(),
            "vendor/alpha/src/legacy/mod.rs: unused_pub_item `gone` in `crate::legacy` (value \
             namespace)"
                .to_string(),
        ],
        "and every entry recording one is stale, item kinds included"
    );
    assert_eq!(
        suppressed(&analysis),
        1,
        "`beta` did not move, so its entry still matches: the failure is scoped \
         to the package the event happened to"
    );
}

/// The unconfigured run the two cases below are measured against: two dead
/// files, nothing else, and no baseline.
#[test]
fn the_deadfiles_fixture_reports_two_unrelated_dead_files() {
    let analysis = analyze_fixture("deadfiles");
    assert_eq!(
        all_reported(&analysis),
        vec![
            (
                FindingKind::DeadFile,
                "src/attic/dropped.rs".to_string(),
                String::new()
            ),
            (
                FindingKind::DeadFile,
                "src/spare.rs".to_string(),
                String::new()
            ),
        ]
    );
    assert!(
        analysis.baseline.is_none(),
        "no `baseline` key and no file at the default location is no baseline"
    );
}

/// Baselining one dead file leaves an unrelated one reported. Both are where
/// the baseline says they are, so this is the exact key answering and it has
/// always held; it is here as the control for the case below, where the only
/// difference is that the recorded path is gone.
#[test]
fn one_of_two_dead_files_baselined_leaves_the_other_reported() {
    let analysis = analyze_configured("deadfiles", "pair.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::DeadFile,
            "src/spare.rs".to_string(),
            String::new()
        )]
    );
    assert_eq!(suppressed(&analysis), 1);
    assert!(
        stale_keys(&analysis).is_empty(),
        "{:?}",
        stale_keys(&analysis)
    );
}

/// The case any content-free rule gets wrong, which is why there is none. One
/// leftover entry — a dead file that was deleted — and one leftover finding — a
/// dead file that is new — is the same evidence a moved dead file leaves, and a
/// dead file records no name and no module for anything to tell them apart
/// with. Pairing them would silence `src/spare.rs`, so the run reports it and
/// names the entry stale: noise, which is the direction this pass falls back
/// in.
#[test]
fn an_unrelated_dead_file_is_not_read_as_a_move_of_a_baselined_one() {
    let analysis = analyze_configured("deadfiles", "unrelated.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::DeadFile,
            "src/spare.rs".to_string(),
            String::new()
        )],
        "the new dead file is news, not the old one in a new place"
    );
    assert!(analysis.has_denied(), "so it fails the run");
    assert_eq!(
        stale_keys(&analysis),
        vec!["src/deleted.rs: dead_file".to_string()],
        "and the deleted one's entry is stale"
    );
    assert_eq!(
        suppressed(&analysis),
        1,
        "the dead file that did not move is still suppressed by the exact key, \
         which is what leaves exactly one leftover on each side"
    );
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

/// The round trip through both writing flags, on the field the key gained:
/// `--write-baseline` records the module of every item finding, deleting one
/// twin's entry reports that twin and nothing else, and `--prune-baseline`
/// carries the surviving modules back out to the file unchanged.
#[test]
fn both_writing_flags_round_trip_the_module_of_every_entry() {
    let dir = scratch_fixture("module-round-trip");
    let baseline = dir.join("deadwood-baseline.json");

    let (code, _) = run_binary(&dir, &["--write-baseline"]);
    assert_eq!(code, Some(0));

    let modules = |path: &Path| -> Vec<Option<String>> {
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        file["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                entry
                    .get("module")
                    .map(|module| module.as_str().unwrap().to_string())
            })
            .collect()
    };
    assert_eq!(
        modules(&baseline),
        vec![
            // The two dependency findings and the dead file name no module,
            // and the key has to record that as an absence rather than
            // inventing one.
            None,
            None,
            Some("crate".to_string()),
            Some("crate::alpha".to_string()),
            Some("crate::beta".to_string()),
            None,
        ],
        "the written file records the module of exactly the kinds that have one"
    );

    // Drop `crate::alpha`'s entry, as a developer accepting one twin and not
    // the other would. Only that twin comes back, and it comes back at its own
    // line rather than the one left in the file.
    let mut file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    file["findings"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| entry["module"] != "crate::alpha");
    std::fs::write(&baseline, serde_json::to_string(&file).unwrap()).unwrap();

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(
        code,
        Some(1),
        "the un-recorded twin fails the run: {stdout}"
    );
    assert!(stdout.contains("1 finding(s) in workspace"), "{stdout}");
    assert!(stdout.contains("src/lib.rs:11:"), "{stdout}");
    assert!(
        stdout.contains("5 finding(s) suppressed"),
        "and `crate::beta`'s twin is still covered:\n{stdout}"
    );
    assert!(
        !stdout.contains("Stale baseline entries"),
        "nothing went stale:\n{stdout}"
    );

    let (code, stdout) = run_binary(&dir, &["--prune-baseline"]);
    assert_eq!(code, Some(1), "{stdout}");
    assert_eq!(
        modules(&baseline),
        vec![
            None,
            None,
            Some("crate".to_string()),
            Some("crate::beta".to_string()),
            None,
        ],
        "pruning re-serializes, and every surviving module survives it"
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
            ("app/src/inline.rs".to_string(), "only_an_inline_gate"),
            ("app/src/inline.rs".to_string(), "only_an_outline_gate"),
            ("app/src/inline.rs".to_string(), "only_a_nested_inline_gate"),
            ("app/src/inline.rs".to_string(), "only_an_inline_gate_use"),
            ("app/src/inline.rs".to_string(), "behind_an_all_gate"),
            ("app/tests/support/mod.rs".to_string(), "from_target"),
            ("probe/src/hidden.rs".to_string(), "declared"),
            ("probe/src/hidden.rs".to_string(), "undeclared"),
        ],
    );
}

/// The claim phase 14 exists for. `#[cfg(test)] mod gated { ... }` and
/// `#[cfg(test)] mod outline;` are one construct written two ways, and the
/// entry point inside each is an `#[allow(dead_code)]` function — neither
/// `#[test]` nor `#[bench]`, so the attribute cannot answer and the gate on
/// the `mod` is the only thing that can. Asserting them together is what makes
/// a fix that moves one and forgets the other fail here.
#[test]
fn the_inline_and_out_of_line_spellings_of_a_cfg_test_mod_agree() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(names.contains(&"only_an_inline_gate"), "{names:?}");
    assert!(names.contains(&"only_an_outline_gate"), "{names:?}");
}

/// Confinement accumulates downward and never lifts, so an *ungated* `mod`
/// inside a test-only one is test code too — the rule phase 7 wrote down for
/// files, holding for blocks.
#[test]
fn a_module_nested_inside_a_test_only_inline_module_is_test_code_too() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(names.contains(&"only_a_nested_inline_gate"), "{names:?}");
}

/// The consumer of `test_context` that is easiest to forget: `add_use` reads
/// it too, so a `#[allow(unused)] use` inside an inline `#[cfg(test)] mod` is
/// a test entry point and what it imports is reached only from test code. A
/// fix that moves `collect_items` and leaves `add_use` behind gives two
/// spellings that disagree one level down, and fails here.
#[test]
fn an_unused_use_inside_an_inline_test_module_is_a_test_entry_point() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(names.contains(&"only_an_inline_gate_use"), "{names:?}");
}

/// The negative half, which is what reusing `cfg::Gates::test_only` buys over
/// matching `#[cfg(test)]` by shape. `any(test, unix)` holds in a build with
/// no tests in it, so the module is not confined to one; `not(test)` is the
/// gate that holds only outside a test build. Both root what is under them
/// normally, and what they reach stays out of the kind.
#[test]
fn only_a_gate_that_holds_nowhere_but_a_test_build_makes_its_module_test_code() {
    let analysis = analyze_configured("testonly", "warn.toml");
    let names: Vec<&str> = test_only(&analysis).into_iter().map(|(_, n)| n).collect();
    assert!(!names.contains(&"behind_an_any_gate"), "{names:?}");
    assert!(!names.contains(&"behind_a_not_test_gate"), "{names:?}");
    // The comparison that gives those two their teeth, and the shape a
    // syntactic `#[cfg(test)]` match would miss in the other direction:
    // `all(test, feature = "extra")` narrows a test build and is still one.
    assert!(names.contains(&"behind_an_all_gate"), "{names:?}");
    assert!(names.contains(&"only_an_inline_gate"), "{names:?}");
}

/// Reporting *more* is the direction of this fix, so the other direction needs
/// pinning: a `pub(crate)` item reached from one of the new test entry points
/// is not suddenly a finding of some other kind. It is already the fix a
/// reported item is told to make.
#[test]
fn a_crate_private_item_behind_a_test_gated_entry_point_is_not_reported() {
    let analysis = analyze_configured("testonly", "warn.toml");
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("already_crate_private")),
        "{:?}",
        analysis.findings
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
    assert!(stdout.contains("9 finding(s)"), "{stdout}");

    let (code, stdout) = run_binary(
        &dir,
        &["--config", config.to_str().unwrap(), "--write-baseline"],
    );
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("Wrote 9 finding(s)"), "{stdout}");
    let recorded = std::fs::read_to_string(dir.join("deadwood-baseline.json")).unwrap();
    assert!(recorded.contains("\"test_only_item\""), "{recorded}");

    let (code, stdout) = run_binary(&dir, &["--config", config.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{stdout}");
    assert!(stdout.contains("No issues found."), "{stdout}");
    assert!(stdout.contains("9 finding(s) suppressed"), "{stdout}");
}

// ---------------------------------------------------------------------------
// The namespace half of the match key (#30).
// ---------------------------------------------------------------------------

/// Every namespace a finding can name, as `(name, namespace)` in report order.
fn namespaces(analysis: &Analysis) -> Vec<(&str, &str)> {
    analysis
        .findings
        .iter()
        .map(|f| {
            (
                f.name.as_deref().unwrap_or_default(),
                match f.namespace {
                    Some(deadwood::Namespace::Type) => "type",
                    Some(deadwood::Namespace::Value) => "value",
                    Some(deadwood::Namespace::Both) => "both",
                    None => "",
                },
            )
        })
        .collect()
}

/// The population the whole phase is about, at the top: a braced struct and a
/// function of its name in one module are two findings that agree in every
/// other field the key looks at.
#[test]
fn a_type_and_a_value_of_one_name_in_one_module_are_two_findings() {
    let analysis = analyze_fixture("namespace");
    assert_eq!(
        namespaces(&analysis),
        vec![
            ("Group", "type"),
            ("Group", "value"),
            ("Limb", "type"),
            ("Limb", "type"),
            ("Shape", "both"),
            ("Shape", "value"),
            ("parse", "value"),
        ]
    );
    let groups: Vec<_> = analysis
        .findings
        .iter()
        .filter(|f| f.name.as_deref() == Some("Group"))
        .collect();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].file, groups[1].file);
    assert_eq!(groups[0].module, groups[1].module);
    assert_eq!(groups[0].kind, groups[1].kind);
    assert_ne!(
        groups[0].namespace, groups[1].namespace,
        "and the namespace is the only field left that separates them"
    );
}

/// The headline case of #30: baseline one of the two and the other is news.
#[test]
fn an_entry_naming_a_namespace_leaves_its_same_named_neighbour_reported() {
    let analysis = analyze_configured("namespace", "separated.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::UnusedPubItem,
            "src/lib.rs".to_string(),
            "Group".to_string()
        )]
    );
    assert_eq!(
        analysis.findings[0].namespace,
        Some(deadwood::Namespace::Value),
        "the half that was not recorded"
    );
    assert!(analysis.has_denied(), "so it fails the run");
    assert_eq!(suppressed(&analysis), 6);
    assert!(
        stale_keys(&analysis).is_empty(),
        "and nothing went stale: {:?}",
        stale_keys(&analysis)
    );
}

/// The half that must not move, and the acceptance criterion #30 names: two
/// `cfg`-alternative spellings of one item are in one namespace, so one entry
/// still covers both. Splitting them would report a second finding about one
/// item with one fix.
#[test]
fn two_cfg_alternative_definitions_of_one_item_stay_covered_by_one_entry() {
    let analysis = analyze_configured("namespace", "alternatives.toml");
    let limbs = namespaces(&analysis)
        .into_iter()
        .filter(|(name, _)| *name == "Limb")
        .count();
    assert_eq!(limbs, 0, "both halves of `Limb` are covered by one entry");
}

/// The decision `Namespace::Both` is: a unit or tuple struct binds a value of
/// its own name, so it *overlaps* the value opposite it and one entry covers
/// both. That is deliberate rather than a gap — the two cannot be compiled
/// together (E0428), so what the key leaves joined is two spellings of one
/// item, which is the case one entry is right for.
#[test]
fn a_unit_struct_and_the_value_it_alternates_with_stay_covered_by_one_entry() {
    let analysis = analyze_configured("namespace", "alternatives.toml");
    let shapes = namespaces(&analysis)
        .into_iter()
        .filter(|(name, _)| *name == "Shape")
        .count();
    assert_eq!(
        shapes, 0,
        "the `both` entry covers the value half as well as the struct"
    );
    assert_eq!(
        all_reported(&analysis)
            .into_iter()
            .map(|(_, _, name)| name)
            .collect::<Vec<_>>(),
        vec!["Group", "Group", "parse"],
        "and only the pairs nothing joins are left"
    );
    assert_eq!(suppressed(&analysis), 4);
    assert!(stale_keys(&analysis).is_empty());
}

/// The upgrade path, from a file checked in as the previous release wrote it:
/// every entry names a module, none names a namespace, and all seven findings
/// stay suppressed with no edit. A round trip through one binary would prove
/// nothing about the binary that wrote the file last week.
#[test]
fn a_baseline_written_before_the_namespace_field_still_matches() {
    let recorded =
        std::fs::read_to_string(fixtures().join("namespace/legacy-baseline.json")).unwrap();
    assert!(recorded.contains("\"module\""), "{recorded}");
    assert!(!recorded.contains("namespace"), "{recorded}");

    let analysis = analyze_configured("namespace", "legacy.toml");
    assert!(analysis.findings.is_empty(), "{:?}", analysis.findings);
    assert!(!analysis.has_denied(), "so the run exits 0");
    assert_eq!(
        suppressed(&analysis),
        7,
        "four entries, and between them they cover both namespaces of every name"
    );
    assert!(
        stale_keys(&analysis).is_empty(),
        "{:?}",
        stale_keys(&analysis)
    );
}

/// A `mod` declaration is in the type namespace and is never reportable, so
/// `pub mod parse;` beside `pub fn parse()` is a namespace collision that
/// produces one finding rather than two — and no key for two findings to share.
/// This is why the type-and-value population is the size it is.
#[test]
fn a_pub_mod_is_not_reported_so_it_shares_no_key_with_a_same_named_fn() {
    let analysis = analyze_fixture("namespace");
    let parses: Vec<_> = namespaces(&analysis)
        .into_iter()
        .filter(|(name, _)| *name == "parse")
        .collect();
    assert_eq!(parses, vec![("parse", "value")]);
}

/// The compatibility claim the second additive field rests on: it is written on
/// exactly the entries `module` is written on, so no baseline gains a field
/// that would newly break an older Deadwood — every file it appears in already
/// carried a `module` that did.
#[test]
fn every_finding_that_names_a_module_names_a_namespace() {
    let mut seen = 0;
    for entry in std::fs::read_dir(fixtures()).unwrap() {
        let path = entry.unwrap().path();
        if !path.is_dir() {
            continue;
        }
        let Ok(analysis) = analyze(&path, None) else {
            continue;
        };
        for finding in &analysis.findings {
            assert_eq!(
                finding.module.is_some(),
                finding.namespace.is_some(),
                "{finding:?}"
            );
            seen += finding.namespace.is_some() as usize;
        }
    }
    assert!(seen > 0, "and some fixture has to produce one");
}

/// A relocation is the identity a move preserves, and the namespace is
/// deliberately not part of it: an entry written before the field existed has
/// none, and requiring one would un-baseline every moved file in every baseline
/// already committed. An entry that *does* name one relocates exactly as its
/// namespace-free twin beside it does.
#[test]
fn an_entry_naming_a_namespace_relocates_across_a_moved_file() {
    let analysis = analyze_configured("moved", "namespaced.toml");
    assert!(
        analysis.findings.is_empty(),
        "{:?}",
        all_reported(&analysis)
    );
    assert_eq!(suppressed(&analysis), 6);
    assert!(
        stale_keys(&analysis).is_empty(),
        "{:?}",
        stale_keys(&analysis)
    );
}

/// Where the namespace does reach the second pass: on the one pairing the
/// identity proposes. A struct that went away and a function of its name that
/// appeared are two events, not one move, so the pass declines — noise, which
/// is what it falls back to whenever the evidence runs out.
#[test]
fn a_moved_entry_whose_namespace_disagrees_is_not_read_as_a_move() {
    let analysis = analyze_configured("moved", "renamespaced.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::UnusedPubItem,
            "alpha/src/legacy/mod.rs".to_string(),
            "gone".to_string()
        )]
    );
    assert_eq!(
        stale_keys(&analysis),
        vec!["alpha/src/legacy.rs: unused_pub_item `gone` in `crate::legacy` (type namespace)"]
    );
    assert_eq!(suppressed(&analysis), 5);
}

/// Both writing flags carry the new field through the file, and a baseline the
/// tool wrote is a baseline it reads back with nothing changed.
#[test]
fn both_writing_flags_round_trip_the_namespace_of_every_entry() {
    let dir = scratch_unconfigured("namespace", "namespace-round-trip");
    let baseline = dir.join("deadwood-baseline.json");

    let (code, _) = run_binary(&dir, &["--write-baseline"]);
    assert_eq!(code, Some(0));

    let recorded = |path: &Path| -> Vec<(String, String)> {
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        file["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["name"].as_str().unwrap().to_string(),
                    entry["namespace"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    assert_eq!(
        recorded(&baseline),
        vec![
            ("Group".to_string(), "type".to_string()),
            ("Group".to_string(), "value".to_string()),
            ("Limb".to_string(), "type".to_string()),
            ("Limb".to_string(), "type".to_string()),
            ("Shape".to_string(), "both".to_string()),
            ("Shape".to_string(), "value".to_string()),
            ("parse".to_string(), "value".to_string()),
        ]
    );

    // Drop the value half of `Group`, as a developer accepting the struct and
    // not the shim beside it would. Only that half comes back.
    let mut file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&baseline).unwrap()).unwrap();
    file["findings"]
        .as_array_mut()
        .unwrap()
        .retain(|entry| !(entry["name"] == "Group" && entry["namespace"] == "value"));
    std::fs::write(&baseline, serde_json::to_string(&file).unwrap()).unwrap();

    let (code, stdout) = run_binary(&dir, &[]);
    assert_eq!(
        code,
        Some(1),
        "the un-recorded half fails the run: {stdout}"
    );
    assert!(stdout.contains("1 finding(s) in workspace"), "{stdout}");
    assert!(stdout.contains("src/lib.rs:19:"), "{stdout}");
    assert!(
        stdout.contains("6 finding(s) suppressed"),
        "and the struct is still covered:\n{stdout}"
    );
    assert!(
        !stdout.contains("Stale baseline entries"),
        "nothing went stale:\n{stdout}"
    );

    let (code, stdout) = run_binary(&dir, &["--prune-baseline"]);
    assert_eq!(code, Some(1), "{stdout}");
    assert_eq!(
        recorded(&baseline),
        vec![
            ("Group".to_string(), "type".to_string()),
            ("Limb".to_string(), "type".to_string()),
            ("Limb".to_string(), "type".to_string()),
            ("Shape".to_string(), "both".to_string()),
            ("Shape".to_string(), "value".to_string()),
            ("parse".to_string(), "value".to_string()),
        ],
        "pruning re-serializes, and every surviving namespace survives it"
    );
}

/// The shape `windows-sys` has, and the whole of what
/// [#39](https://github.com/rlorenzo/deadwood/issues/39) is: a crate root that
/// reaches its module tree through `include!("...")`. Every file under it is
/// compiled by the build that actually happens, so none of them is dead — and
/// that includes the ones the included file declares with `mod`, which is 245
/// of the corpus's 246 and the half a fix that only marks the named file
/// misses.
#[test]
fn a_module_tree_reached_only_through_an_include_is_not_dead() {
    let analysis = analyze_fixture("included");

    let dead: Vec<String> = reported(&analysis, FindingKind::DeadFile)
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    for spliced in [
        // The file the `include!` names.
        "src/tree/mod.rs",
        // Declared by it with an ordinary `mod`, which is the corpus's shape.
        "src/tree/branch.rs",
        // And one level further, where the ordinary rules have resumed.
        "src/tree/branch/twig.rs",
    ] {
        assert!(
            !dead.contains(&spliced.to_string()),
            "`{spliced}` is compiled through the `include!`: {dead:?}"
        );
    }
}

/// A `mod` declared inside an included file resolves beside **that file**, not
/// beside the file the `include!` was written in. Both layouts are in the
/// fixture and only one of them compiles: `src/tree/branch.rs` is what rustc
/// loads for `pub mod branch;` in `src/tree/mod.rs`, and `src/branch.rs` is
/// the file a fix that took the includer's directory would have spared
/// instead.
#[test]
fn a_mod_inside_an_included_file_resolves_beside_that_file_not_beside_the_includer() {
    let analysis = analyze_fixture("included");

    let dead: Vec<String> = reported(&analysis, FindingKind::DeadFile)
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    assert!(
        dead.contains(&"src/branch.rs".to_string()),
        "nothing compiles `src/branch.rs`: {dead:?}"
    );
    assert!(
        !dead.contains(&"src/tree/branch.rs".to_string()),
        "`src/tree/branch.rs` is the file `pub mod branch;` loads: {dead:?}"
    );
}

/// Every dead-file phase so far could only add noise; this one takes findings
/// away, so the failure to avoid is exonerating a file that really is dead.
/// `src/attic.rs` sits beside a spliced tree and is reached by nothing, and
/// the exemption must not leak to it.
#[test]
fn a_dead_file_beside_an_included_tree_is_still_reported() {
    let analysis = analyze_fixture("included");

    assert_eq!(
        reported(&analysis, FindingKind::DeadFile),
        vec![
            ("src/attic.rs".to_string(), ""),
            ("src/branch.rs".to_string(), ""),
        ],
        "exactly two files in the fixture are reachable by nothing"
    );
}

/// The boundary this phase stops at, named so that moving it is a decision
/// rather than a diff. An `include!`-ed file is evidence that it was reached
/// and evidence of nothing else: its items take no part in resolution, so
/// `never_named` in the spliced tree is not reported even though nothing in
/// the workspace names it.
///
/// `reached_both_ways` is the control, and the reason this test cannot pass by
/// resolution simply not running: it is declared by `src/lib.rs` as an
/// ordinary module *and* from inside the spliced file, the `mod` walk drains
/// first, and so it is analyzed and reported.
#[test]
fn an_included_files_items_take_no_part_in_resolution() {
    let analysis = analyze_fixture("included");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedPubItem),
        vec![("src/dual.rs".to_string(), "reached_both_ways")],
        "measured before choosing: admitting the spliced items would report \
         132,414 unused public items in `windows-sys` alone, where it reports \
         10 today"
    );
}

/// A crate named only from inside a spliced tree is still a used dependency.
///
/// The dependency check never sees those files through the module tree — the
/// boundary above keeps them out of it — so they have to go on being read the
/// way every file no `mod` declaration names is read. Getting this wrong is
/// this phase inventing an `unused_dependency` finding while it removes
/// `dead_file` ones, which would be a poor trade.
#[test]
fn a_dependency_named_only_inside_a_spliced_tree_is_still_a_used_dependency() {
    let analysis = analyze_fixture("included");

    assert_eq!(
        reported(&analysis, FindingKind::UnusedDependency),
        Vec::new(),
        "`splicedep` is named in `src/tree/branch.rs` and nowhere else"
    );
}

/// An `include!` written inside an inline module takes its path from the
/// *file* it is written in, not from the directory that module's own `mod`
/// declarations would resolve in: `mod inner { include!("tree/twiglet.rs"); }`
/// in `src/lib.rs` is `src/tree/twiglet.rs`, never `src/inner/tree/twiglet.rs`.
/// Only the module path its items land under is the inline module's.
#[test]
fn an_include_inside_an_inline_module_resolves_from_the_file_it_is_written_in() {
    let analysis = analyze_fixture("included");

    let dead: Vec<String> = reported(&analysis, FindingKind::DeadFile)
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    assert!(
        !dead.contains(&"src/tree/twiglet.rs".to_string()),
        "the file the inline module splices in is compiled: {dead:?}"
    );
}

/// An `include!` the configured matrix rules out is not followed, and the
/// files it would have reached fall back to the answer they get today rather
/// than being spared.
///
/// It takes nothing with it into the excluded set either, unlike a `mod` the
/// matrix rules out: an included file's children are its own *directory's*, so
/// for `include!("gen.rs")` at a crate root that directory is the whole of
/// `src/`, and excluding it would suppress every genuinely dead file beside
/// it. Under the default matrix every platform is analyzed, so this costs
/// nothing unless a project narrows the matrix itself.
#[test]
fn an_include_the_matrix_rules_out_is_not_followed() {
    let default = analyze_fixture("included");
    let dead: Vec<String> = reported(&default, FindingKind::DeadFile)
        .into_iter()
        .map(|(file, _)| file)
        .collect();
    assert!(
        !dead.contains(&"src/winonly/mod.rs".to_string()),
        "`#[cfg(windows)] include!` is part of some build the default matrix \
         analyzes: {dead:?}"
    );

    let narrowed = analyze_configured("included", "linux-only.toml");
    assert_eq!(
        reported(&narrowed, FindingKind::DeadFile),
        vec![
            ("src/attic.rs".to_string(), ""),
            ("src/branch.rs".to_string(), ""),
            ("src/winonly/mod.rs".to_string(), ""),
        ],
        "a matrix with no Windows in it does not follow the `include!`, and \
         the two files that were dead before still are"
    );
}

/// A chain of `include!`s is followed as deep as `src/deps.rs` follows one and
/// no deeper, because one crate read to two depths is two readers that
/// disagree about the same file. Nine deep is not the corpus's shape —
/// `windows-sys` has exactly one `include!` — it is the guard.
///
/// Stopping short costs a finding rather than inventing an exemption: the file
/// past the cap goes on being reported, which is the direction this phase's
/// risk runs in.
#[test]
fn an_include_chain_is_followed_as_deep_as_the_dependency_reader_follows_one() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/includedeep");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert_eq!(
        reported(&analysis, FindingKind::DeadFile),
        vec![("src/chain/i9.rs".to_string(), "")],
        "eight `include!`s deep is followed, the ninth is not"
    );
    // The other reader stops in the same place, and says so out loud.
    assert!(
        analysis.warnings.iter().any(|w| {
            w.contains("unused-dependency check skipped")
                && w.contains("code is pulled in with `include!`")
        }),
        "the cap is one constant, and `src/deps.rs` warns on it: {:?}",
        analysis.warnings
    );
}

/// `include!(concat!(env!("OUT_DIR"), "/generated.rs"))` names a file only a
/// build knows. `src/deps.rs` already answers that construct — skip with a
/// warning rather than guess — and the module tree adds no second policy for
/// it: no warning of its own, and no exemption either.
///
/// The direction matters. A package whose `include!` we cannot read must keep
/// reporting the dead files it reports today, not start suppressing them on
/// the suspicion that the file we could not read might name them. `serde` and
/// `serde_core` are written this way and contribute 36 of the corpus's dead
/// files.
#[test]
fn an_include_whose_path_only_a_build_knows_still_reports_the_packages_dead_files() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/includegen");
    let analysis = analyze(&fixture, None).expect("analysis should succeed on the fixture");

    assert_eq!(
        reported(&analysis, FindingKind::DeadFile),
        vec![("src/orphan.rs".to_string(), "")],
        "an unreadable `include!` spares nothing"
    );
    for check in ["unused-dependency", "misplaced-dependency"] {
        assert!(
            analysis.warnings.iter().any(|w| {
                w.contains(&format!("{check} check skipped"))
                    && w.contains("code is pulled in with `include!`")
            }),
            "the skip is surfaced rather than silent: {:?}",
            analysis.warnings
        );
    }
    assert!(
        !analysis
            .warnings
            .iter()
            .any(|w| w.contains("dead-file check skipped")),
        "and it is not a reason to stop reporting dead files: {:?}",
        analysis.warnings
    );
}

// ---------------------------------------------------------------------------
// What a `use` alias binds (#37).
// ---------------------------------------------------------------------------

/// Every finding of `kind` in the fixture, as `(name, namespace)` in report
/// order.
fn named_namespaces(analysis: &Analysis, kind: FindingKind) -> Vec<(&str, deadwood::Namespace)> {
    analysis
        .findings
        .iter()
        .filter(|finding| finding.kind == kind)
        .map(|finding| {
            (
                finding.name.as_deref().unwrap_or_default(),
                finding
                    .namespace
                    .expect("an item finding names a namespace"),
            )
        })
        .collect()
}

/// The resolving table end to end: one dead re-export per shape of target, and
/// the namespace each one records printed in the report rather than only
/// asserted in a unit test.
///
/// Before this phase every line here said `both`.
#[test]
fn a_reported_re_export_names_the_namespaces_its_target_binds() {
    use deadwood::Namespace::{Both, Type, Value};

    let analysis = analyze_fixture("aliases");
    assert_eq!(
        named_namespaces(&analysis, FindingKind::UnusedReexport),
        vec![
            // A braced struct is a type alone, and a rename does not change it.
            ("Braced", Type),
            ("Renamed", Type),
            // A unit struct binds a constructor value of its own name.
            ("Sole", Both),
            ("plain", Value),
            // A module, which is the corpus's only narrowing instance.
            ("sub", Type),
            // A name binding a type *and* a value: the union, not a pick.
            ("Twinned", Both),
            // A group is a question per leaf, and these two share a line.
            ("Listed", Type),
            ("tallied", Value),
            // The two refusals, which keep the value every alias had before.
            ("Outside", Both),
            ("Veiled", Both),
        ]
    );
}

/// The corpus's only instance, and the only finding this phase changes: `pub
/// use tucked::inner;` in the `globs` fixture, which names a module.
///
/// It is here rather than only in the new fixture because it is the one place
/// in the corpus where the phase moves a value a user could already have
/// baselined.
#[test]
fn the_one_re_export_of_a_module_in_the_corpus_narrows_to_the_type_namespace() {
    let analysis = analyze_fixture("globs");
    let inner: Vec<_> = named_namespaces(&analysis, FindingKind::UnusedReexport)
        .into_iter()
        .filter(|(name, _)| *name == "inner")
        .collect();
    assert_eq!(inner, vec![("inner", deadwood::Namespace::Type)]);
}

/// The headline, on the one finding kind that has this collision.
///
/// `unused_reexport` and `unused_pub_item` are two kinds and so two keys
/// whatever the namespace says, so a `pub use` and a `pub fn` of one name are
/// already separate entries there. `test_only_item` is the single kind both are
/// reported under — and there the alias claiming `both` overlapped the
/// function's `value`, so an entry naming one covered the other.
///
/// The entry records the *value* half; the re-export beside it binds a braced
/// struct, so it is in the type namespace and is news. Under the release before
/// this phase the same file and the same fixture report nothing at all.
#[test]
fn an_entry_naming_the_value_half_leaves_the_re_export_beside_it_reported() {
    let analysis = analyze_configured("aliases", "collision.toml");
    assert_eq!(
        all_reported(&analysis),
        vec![(
            FindingKind::TestOnlyItem,
            "src/shared.rs".to_string(),
            "Braced".to_string()
        )]
    );
    assert_eq!(
        analysis.findings[0].namespace,
        Some(deadwood::Namespace::Type),
        "the alias binds what its target binds, and its target is braced"
    );
    assert!(
        analysis.findings[0]
            .message
            .contains("`pub use` re-export of `Braced`"),
        "and it is the re-export rather than the struct: {}",
        analysis.findings[0].message
    );
    assert_eq!(suppressed(&analysis), 5);
    assert!(
        stale_keys(&analysis).is_empty(),
        "{:?}",
        stale_keys(&analysis)
    );
}

/// The half that must not move, and the shape four of the corpus's five
/// re-export findings have: an alias to a *unit* struct really does bind both
/// namespaces, so one entry still covers the value opposite it.
///
/// A regression that narrowed every alias — to its target's type half, say —
/// would report `Sole` here and un-baseline a re-export already accepted, which
/// is the loud failure the whole design is shaped around.
#[test]
fn a_re_export_of_a_unit_struct_stays_covered_by_one_entry() {
    let analysis = analyze_configured("aliases", "collision.toml");
    assert!(
        !analysis
            .findings
            .iter()
            .any(|finding| finding.name.as_deref() == Some("Sole")),
        "the `both` entry covers the function opposite the alias: {:?}",
        all_reported(&analysis)
    );
}

/// The upgrade path, from a file the previous release wrote with
/// `--write-baseline` and checked in unedited.
///
/// Twelve of its entries record `both` for an alias and seven of those are
/// aliases this phase narrows — so the file is exactly the case that would bite
/// if narrowing were not absorbed by the overlap rule. Nothing is stale and
/// nothing is reported: a recorded `both` covers a reported `type` or `value`,
/// which is why this phase can change what the tool writes without touching
/// what it already accepted.
#[test]
fn a_baseline_written_before_aliases_resolved_still_matches() {
    let recorded: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("aliases/legacy-baseline.json")).unwrap(),
    )
    .unwrap();
    let entry = recorded["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "sub")
        .expect("the module re-export is in the file");
    assert_eq!(
        entry["namespace"], "both",
        "the old binary could say nothing else about an alias"
    );

    let analysis = analyze_configured("aliases", "legacy.toml");
    assert_eq!(
        named_namespaces(&analyze_fixture("aliases"), FindingKind::UnusedReexport)
            .into_iter()
            .find(|(name, _)| *name == "sub"),
        Some(("sub", deadwood::Namespace::Type)),
        "and this binary says `type` about the same alias"
    );
    assert!(
        analysis.findings.is_empty(),
        "which changes nothing about what the file covers: {:?}",
        all_reported(&analysis)
    );
    assert_eq!(suppressed(&analysis), 23);
    assert!(
        stale_keys(&analysis).is_empty(),
        "{:?}",
        stale_keys(&analysis)
    );
}

/// One file reached under two spellings, because a symlinked directory gives
/// it two. serde symlinks `serde/src/core` at `serde_core/src`, so the crate
/// root that `serde_core` compiles as `serde_core/src/lib.rs` is walked again
/// as `serde/src/core/lib.rs` while `serde` builds — and was reported dead,
/// against a package that never claimed it.
///
/// Built here rather than checked in: a committed symlink is not a symlink on
/// a Windows checkout without `core.symlinks`, and a fixture that quietly
/// becomes a text file is a test that quietly stops testing.
#[test]
#[cfg(unix)]
fn a_file_reached_through_a_symlink_is_not_dead_under_its_other_name() {
    let root = std::env::temp_dir().join(format!("deadwood-symlink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let real = root.join("real");
    std::fs::create_dir_all(real.join("src")).unwrap();
    std::fs::create_dir_all(root.join("front")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"real\", \"front\"]\n",
    )
    .unwrap();
    for (package, path) in [("real", "src/lib.rs"), ("front", "lib.rs")] {
        std::fs::write(
            root.join(package).join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
                 [lib]\npath = \"{path}\"\n"
            ),
        )
        .unwrap();
    }
    std::fs::write(real.join("src/lib.rs"), "pub fn used() {}\n").unwrap();
    // `front/` holds only the symlink, so the sole `.rs` file the walk finds
    // under it is `real/src/lib.rs` wearing a different name.
    std::fs::write(root.join("front/lib.rs"), "pub fn other() {}\n").unwrap();
    std::os::unix::fs::symlink(real.join("src"), root.join("front/borrowed")).unwrap();

    let analysis = analyze(&root, None).expect("analysis should succeed");
    let dead: Vec<String> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::DeadFile)
        .map(|f| f.file.to_string_lossy().into_owned())
        .collect();
    std::fs::remove_dir_all(&root).ok();
    assert!(
        dead.is_empty(),
        "`real/src/lib.rs` is a crate root, whichever path arrives at it: {dead:?}"
    );
}

/// A `mod` whose file is named by `#[cfg_attr(.., path = "..")]` rather than
/// by a plain `#[path]`, which serde uses for the only module of
/// `serde_derive_internals` — one arm per build, condition never evaluated.
///
/// Reading neither arm left the module unresolved, and an unresolved module is
/// not a wrong finding but the absence of every finding: the package's checks
/// skipped, and the workspace-wide unused-pub check skipped along with them.
/// On serde that silence hid five real items.
#[test]
fn a_cfg_attr_path_names_the_file_its_mod_resolves_to() {
    let analysis = analyze_fixture("cfgattrpath");

    assert!(
        analysis.warnings.is_empty(),
        "both arms name a real file, so nothing is unresolved: {:?}",
        analysis.warnings
    );
    assert!(
        analysis.findings.is_empty(),
        "the arm this build does not take is spared, not reported dead: {:?}",
        analysis.findings
    );
}

/// A package whose crate root is not under `src/`. `src/` is a convention and
/// the manifest is the authority, so the dead-file check walks the directories
/// the targets actually point at.
///
/// Assuming `src/` does not misreport anything — it reports *nothing*, which
/// is why this went unnoticed until bun, where all 101 crates say `[lib] path
/// = "lib.rs"` and the whole check was silently inert across a million lines.
/// The fixture also pins the two limits that keep such a walk from running
/// away once its root is the package directory itself.
#[test]
fn dead_files_are_found_where_the_manifest_puts_the_crate_root() {
    let analysis = analyze_fixture("flatlayout");

    let mut dead_files: Vec<String> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::DeadFile)
        .map(|f| f.file.to_string_lossy().replace('\\', "/"))
        .collect();
    dead_files.sort();
    assert_eq!(
        dead_files,
        vec![
            // Reported against the nested package, whose own tree decides it
            // — never against the outer one whose root directory contains it.
            "inner/stray.rs",
            // Beside the crate root, and one directory down from it.
            "nested/buried.rs",
            "orphan.rs",
        ],
        "the walk follows the manifest, stops at a nested package, and leaves \
         `tests/` alone: {:?}",
        analysis.findings
    );
}

/// The module tree a macro token stream declares
/// ([#60](https://github.com/rlorenzo/deadwood/issues/60)): tokio's 381
/// dead-file findings were live subtrees behind `cfg_fs!`-style wrappers,
/// serde's a tree written inside a `macro_rules!` body, `rustc_target`'s 330
/// the idents of `supported_targets!`. The `macromods` fixture holds all
/// three shapes; the files they declare are spared, and the genuinely dead
/// file beside them is still the finding it always was.
#[test]
fn a_mod_declared_only_in_a_macro_token_stream_spares_its_file() {
    let analysis = analyze_fixture("macromods");

    let dead_files: Vec<&PathBuf> = analysis
        .findings
        .iter()
        .filter(|f| f.kind == FindingKind::DeadFile)
        .map(|f| &f.file)
        .collect();
    // Two of the fixture's files exist to hold `queue_speculative`'s pair of
    // `#[path]` probe starting points apart, because the pair reads like a
    // belt-and-braces duplicate and is not one. `via_base/inner/UnderBase.rs`
    // is reached only from `base` — the invocation sits inside an inline
    // `mod`, so the expansion resolves from `src/via_base/inner/` — and
    // `BesideDeclarer.rs` only from the declaring file's own directory, where
    // an invocation outside every inline block puts it. Written the other way
    // round neither compiles. Dropping either probe leaves one of them here.
    assert_eq!(
        dead_files,
        vec![&PathBuf::from("src/orphan.rs")],
        "only the file no macro and no `mod` names is dead: {:?}",
        analysis.findings
    );

    // The other half of the treatment: a macro-reached file is spared, but
    // its items are not admitted to resolution — the module path the macro
    // gives them is unknowable without expansion — so the `pub fn` in
    // `wrapped.rs` that nothing references is not a finding either.
    //
    // The paths such a file *writes* are the opposite case, and the assertion
    // below is the one that matters: `wrapped.rs` holds the only reference to
    // `reached_from_macro_mod::only_the_macro_mod_calls_me`, and dropping it
    // would not lose a finding but invent one. Being wrong about a reference
    // costs a finding; being wrong about a definition costs precision, which
    // is why only one of the two halves is admitted.
    assert!(
        reported(&analysis, FindingKind::UnusedPubItem).is_empty(),
        "a macro-reached file defines no findings and silences no references: {:?}",
        analysis.findings
    );
}

/// The md-5 shape ([#62](https://github.com/rlorenzo/deadwood/issues/62))
/// through a workspace member: `app` declares `spoked` by package name and
/// spells its *lib* name (`wheel`) in code. A member's lib rename is visible
/// with or without a resolvable dependency graph, so the entry is never
/// reported unused.
#[test]
fn a_dependency_is_matched_by_its_lib_target_name() {
    let analysis = analyze_fixture("crosscrate");
    assert!(
        !analysis
            .findings
            .iter()
            .any(|f| f.name.as_deref() == Some("spoked")),
        "the lib name `wheel` is what code spells: {:?}",
        analysis.findings
    );
}

/// The same, where it has to survive a workspace whose full resolution
/// *fails*: `libname` declares a crate no registry has, so the lib-name map
/// cannot come from a resolved graph — a member's rename is read from the
/// `--no-deps` view that is always in front of us.
#[test]
fn a_members_lib_rename_survives_an_unresolvable_workspace() {
    let analysis = analyze_fixture("libname");
    assert!(
        reported(&analysis, FindingKind::UnusedDependency).is_empty(),
        "`rim::radius()` is `rim-parts`' evidence, resolvable graph or none: {:?}",
        analysis.findings
    );
}
