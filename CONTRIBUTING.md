# Contributing a plugin

Thanks for helping. A plugin is one directory, `plugins/<name>/`, holding a crate and a `caps.json`.

## Rules

- **Licence.** `MIT OR Apache-2.0`, like the rest of the repository (`license.workspace = true`).
- **Name.** The directory and the crate (`[package] name`) are identical.
- **Crate.** `crate-type = ["cdylib", "lib"]`, depending on `wayhouse-plugin-abi = { workspace = true }`. Set `publish = false`.
- **Capabilities.** `caps.json` next to the crate declares triggers and capabilities (for example `on_timer`, `tick_interval_secs`, `log`, `state`). Ask for the minimum: an operator has to approve every capability, and the host enforces them.
- **Tests required.** Unit tests run natively with `cargo test`. No network and no clock in tests.
- **Bounded work.** The host enforces limits per call; keep a plugin small and allocation-light.
- **Review.** Official plugins are reviewed by a maintainer before merge.

## Workflow

1. Copy `plugins/selftest` and edit it; add a row to the README table.
2. `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
3. `cargo build --release --target wasm32-unknown-unknown --workspace`, then run `wayhouse-plugin-check` on your `.wasm` (see the README).
4. Open a pull request with a conventional-commit title. CI runs the same checks and the conformance check on every plugin.

Agents and automation: see [`AGENTS.md`](AGENTS.md).

## Outside this repository

You do not need to be merged here to share a plugin. Plugins from other repositories are installed at the user's own risk; the sandbox and approved capabilities are the hard wall for all of them.

## Code of conduct and security

The [Code of Conduct](CODE_OF_CONDUCT.md) applies to every project space. Report vulnerabilities privately as described in [`SECURITY.md`](SECURITY.md).
