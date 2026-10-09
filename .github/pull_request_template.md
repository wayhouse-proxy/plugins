## What and why

<!-- One or two sentences. Link the issue with "Closes #N" if there is one. -->

## Checklist

- [ ] PR title is a conventional commit (`feat(selftest): ...`, `fix: ...`, `docs: ...`)
- [ ] `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] New or changed plugin: built for `wasm32-unknown-unknown` and `wayhouse-plugin-check` passes
- [ ] `caps.json` declares only the capabilities the plugin really needs
- [ ] README table updated if a plugin was added or its capabilities changed
