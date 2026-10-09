# AGENTS.md

Guidance for any AI coding agent working in this repository, following the [agents.md](https://agents.md) convention. Humans: see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## What this repo is

The official **plugins** for [wayhouse](https://github.com/wayhouse-proxy/wayhouse): WASM modules that react to a trigger (today: a timer) and act only through capabilities an operator approved. They are not *sniffers* (those live in [`wayhouse-proxy/sniffers`](https://github.com/wayhouse-proxy/sniffers) and run on the data path). The plugin host, the ABI crate (`wayhouse-plugin-abi`) and the conformance checker (`wayhouse-plugin-check`) live in the wayhouse repository; this repo only holds plugins. Design: `docs/plugins.md` and `docs/superpowers/specs/` in wayhouse.

## Layout

```
Cargo.toml                workspace; members = plugins/*; pins wayhouse-plugin-abi by git rev
rust-toolchain.toml       pinned toolchain
plugins/<name>/           one cdylib crate per plugin
  Cargo.toml
  caps.json               capability declaration, embedded as the wayhouse.plugin-caps section
  src/lib.rs
.github/workflows/ci.yml  fmt, clippy, tests, wasm build, conformance check, ci-ok gate
```

`plugins/selftest` is the smallest plugin; copy it when adding one.

## Commands

Run these before every push (CI runs the same, with `RUSTFLAGS=-D warnings`):

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --target wasm32-unknown-unknown --workspace   # needs the wasm32-unknown-unknown target
wayhouse-plugin-check target/wasm32-unknown-unknown/release/<name>.wasm
```

`wayhouse-plugin-check` is installed from the wayhouse revision pinned in `Cargo.toml` (command in the README). Cold builds of the host crates are slow; install the checker once and reuse it. The `protobuf-compiler` package is needed for that install.

## Rules

- **Never skip, disable or weaken a test or CI check** to get green. Fix the cause.
- **One revision, two places.** `wayhouse-plugin-abi`'s `rev` in `Cargo.toml` and `WAYHOUSE_REV` in `ci.yml` must stay identical. Bump both together, and rebuild and re-check every plugin, because the host requires an exact ABI minor match while the major is 0.
- **Capabilities are the hard wall.** A plugin's `caps.json` must request the minimum it needs; do not widen it without saying so in the PR description. Plugins here are still third-party code to operators.
- **Tests run natively** (`cargo test`): no network, no clock, deterministic.
- **Published behaviour:** add or change a plugin's row in the README table in the same PR.
- **Do not edit `CODE_OF_CONDUCT.md`** or the licence files unless asked.

## Git and PRs

- Work on a branch, never directly on `main`.
- PR titles are conventional commits (`feat(selftest): ...`, `fix: ...`, `docs: ...`); the PR title becomes the squash commit message.
- `main` is protected by a merge queue: once CI is green, queue with `gh pr merge <N> --squash --auto`. The required check is `ci-ok`.
- Docs-only PRs need no extra review once CI is green; code PRs get a human review.
