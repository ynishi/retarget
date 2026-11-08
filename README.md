# retarget

Tune a coding agent's harness one setting at a time, and keep only what measurably helps.

retarget watches the telemetry your coding agent's harness already sends,
changes one thing about how the agent runs, compares runs with and without the
change, and keeps it only when the evidence clears a rule fixed in advance — in
the end, a lower cost per successful run. Otherwise it puts the change back.

## Crates

| crate | role |
|---|---|
| `retarget` | Umbrella: the name users depend on and the `retarget` command (`cargo install retarget`). Re-exports the core and holds `infra`, the adapters with real I/O. The command and what only it needs are behind the default `cli` feature, so a library user writes `default-features = false`. |
| `retarget-core` | Core (SDK): `domain` (pure rules and types) and `app` (use cases and their ports). No I/O dependency. |

A server that someone hosts for others becomes a crate of its own,
`retarget-server`, with its own binary, when it is needed.

The layer rules are in `arch-lint.toml` and run with `cargo test`. A crate or layer
that does not fit the project is deleted, together with its `arch-lint.toml` scope.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
