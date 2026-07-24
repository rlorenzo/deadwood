# Environment assessment (2026-07-24 bootstrap)

State of the repository and build environment when Deadwood was bootstrapped,
what was missing, and what was done about it.

## What was found

| Area | State at bootstrap |
| --- | --- |
| Repository | Empty: one initial commit containing only an MIT `LICENSE`. No Cargo project, no source, no CI, no configs. |
| Rust toolchain | `rustc`/`cargo` 1.94.1 stable via rustup, with `clippy` and `rustfmt` components installed. |
| Network | crates.io reachable through the environment's HTTPS proxy (verified with a scratch `cargo fetch` before adding any dependency). |
| GitHub | Access via MCP tools; no `gh` CLI in this environment. |
| Dev tools | No `just`, no task runner assumed. Plain `bash` and `cargo` only. |

## Gaps identified and how each was resolved

| Gap | Resolution |
| --- | --- |
| No Cargo project / workspace metadata | Created single-package project (`lib` + `bin`) — a full cargo workspace split (`deadwood-core`, etc.) is deferred until a second crate is actually needed. |
| No toolchain pin | `rust-toolchain.toml` pins `stable` and requires `clippy` + `rustfmt`; `rust-version = "1.94"` recorded in `Cargo.toml`. |
| No lockfile policy | `Cargo.lock` committed (binary crate) for reproducible builds. |
| No format/lint config | rustfmt defaults (no config file to drift from); clippy `all` warnings enabled via `[lints]` in `Cargo.toml`, enforced as errors in CI/scripts with `-D warnings`; `unsafe_code = "forbid"`. |
| No CI | `.github/workflows/ci.yml`: fmt check, clippy `-D warnings`, tests on push/PR. |
| No dev scripts | `scripts/check.sh` (with `--fix` mode) mirrors CI exactly. |
| No tests or fixtures | Unit tests per module plus an end-to-end test against `tests/fixtures/simple` (a self-contained fixture package with known dead code). |

## Remaining gaps / notes for future work

- **CI is unverified in-environment.** The workflow file is standard
  (dtolnay/rust-toolchain + Swatinem/rust-cache) but this environment cannot
  execute GitHub Actions; verify the first run on GitHub. If the push is
  rejected for missing `workflows` permission, the file must be added through
  the GitHub UI or a token with workflow scope.
- **No release/packaging setup** (crates.io publish, binary releases) — out of
  scope for v0.1, nothing blocks it later.
- **`cargo metadata` dependency at runtime**: Deadwood shells out to `cargo`.
  Fine for a dev tool; revisit only if a cargo-less mode is ever needed.
- **Semantic analysis gap**: nothing in the current environment blocks moving
  to deeper analysis later (e.g. `rustc`-driven or `rust-analyzer`-based
  resolution), but no such integration exists yet; today everything is
  `syn`-level. This is the main known ceiling of the current design.
