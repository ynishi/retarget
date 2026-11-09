# retarget

Find what wastes your coding agent's or swarm's runs, apply the fix that is known for it, and keep it where it fits.

retarget watches the telemetry your coding agent's harness already sends, finds
patterns of waste in it, applies the technique its catalogue knows for each, and
checks that the technique fits you. One that makes things worse is put back and
the next is tried. What counts is the agent's performance: what a successful
task costs, how long it takes, how often it succeeds.

Techniques are found and proven in a Lab, with experiments. The framework — the
core, the adapters, the Lab's machinery and a catalogue built from public facts —
is open here. Data, a tuned catalogue and the features that find patterns plug
in from outside it.

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
