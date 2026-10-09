# wayhouse plugins

Plugins for [wayhouse](https://github.com/wayhouse-proxy/wayhouse): WASM modules that react to a trigger (today: a timer) and act only through capabilities an operator approved. Not to be confused with [sniffers](https://github.com/wayhouse-proxy/sniffers), which run on the proxy data path.

Plugins from this repository are third-party code: install and run them at your own risk. The sandbox and the approved capabilities are the hard wall.

## Plugins

| Plugin | What it is | Capabilities |
| ------ | ---------- | ------------ |
| [`selftest`](plugins/selftest) | Test plugin: counts its ticks in `state` and logs each one. Install it to prove the plugin pipeline works. | `on_timer` (10 s), `log`, `state` (1 KiB) |

## Building

Needs the Rust toolchain from `rust-toolchain.toml` and the `wasm32-unknown-unknown` target.

```sh
cargo test --workspace
cargo build --release --target wasm32-unknown-unknown --workspace
```

The modules land in `target/wasm32-unknown-unknown/release/<name>.wasm`.

## Checking a plugin against the host

`wayhouse-plugin-check` loads a module the way the controller would, calls `init` and `on_timer` once, and exits 1 on any problem. CI runs it on every built plugin. Locally (the revision is the one `Cargo.toml` pins):

```sh
cargo install --locked --git https://github.com/wayhouse-proxy/wayhouse --rev <rev> wayhouse-plugin-host --bin wayhouse-plugin-check
wayhouse-plugin-check target/wasm32-unknown-unknown/release/selftest.wasm
```

## Installing `selftest` on a controller

Start a standalone controller with `--plugins`, then (admin bearer token; see `docs/plugins.md` in wayhouse):

```sh
sha=$(curl -s -H "Authorization: Bearer $TOKEN" --data-binary @target/wasm32-unknown-unknown/release/selftest.wasm http://localhost:PORT/plugins/modules | jq -r .sha256)
curl -s -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d "{\"name\":\"selftest\",\"sha256\":\"$sha\",\"approved\":{\"triggers\":{\"on_timer\":true},\"tick_interval_secs\":10,\"log\":true,\"state\":{\"max_bytes\":1024}},\"enabled\":true}" \
  http://localhost:PORT/plugins
```

Or upload it on the Plugins page of the web UI. After about 10 seconds `GET /plugins/{id}/status` shows `ticks` rising and `selftest tick N` log lines; the counter lives in the plugin's state and keeps counting across controller restarts.

## Layout

One crate per plugin under `plugins/<name>/`, built as `cdylib`, depending on `wayhouse-plugin-abi` (pinned by git revision in the workspace `Cargo.toml`). The capability declaration is a `caps.json` next to the crate, embedded as the `wayhouse.plugin-caps` section.

## Code of conduct

Participation is covered by the [Code of Conduct](CODE_OF_CONDUCT.md). Report problems privately through a GitHub security advisory on this repository.
