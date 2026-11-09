# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Rust developers, and especially maintainers of large Cargo workspaces, who run Deadwood locally or as a CI gate to keep dead code out of their codebase.

## Product Purpose

Deadwood finds dead code that `rustc` and Clippy miss. That covers source files no module tree reaches, `pub` items and re-exports nothing in the workspace uses, and dependencies that are unused or sit in the wrong `Cargo.toml` table. It's a success when a workspace stays free of dead weight without slowing anyone down.

## Positioning

Deadwood is like knip and Fallow, but for Rust. It reads source without compiling, so there's no `cargo build` and no nightly toolchain, and it analyzes the whole rust-lang/rust workspace in seconds. It decides what's dead by resolving paths and walking reachability from target roots, not by counting identifiers. A baseline lets an existing codebase fail only on new findings.

## Operating Context

- A terminal CLI (`deadwood check`) with text or JSON output and exit codes built for CI.
- A composite GitHub Action, listed on GitHub Marketplace as "Deadwood for Rust".
- A crate on crates.io, published as `deadwood-rs`; the binary is named `deadwood`.

## Brand Commitments

- Name: "Deadwood". The Marketplace listing is "Deadwood for Rust" because "Deadwood" was already taken there.
- The Marketplace branding in `action.yml` uses the color orange and the Feather `crosshair` icon.
- Logo surfaces: the GitHub social preview (1280×640), the README header (must work on GitHub's light and dark themes), and a text-only version for the terminal.
- Anti-goals: violent imagery (crosshairs, guns, scopes), rustic or lumberjack imagery, and the generic AI-dev look (glowing gradients, neon circuit lines, hexagon badges). Don't derive from Rust's own logo or Ferris the crab.

## Evidence on Hand

- The README's comparison table against rustc, Clippy, cargo-machete, cargo-udeps and cargo-shear.
- Measured runs on bevy, ruff, helix, bun, serde, tokio, ripgrep and 1265 registry crates; see `docs/HISTORY.md`.
- No logo or visual identity exists yet.
