# retarget — pointers for coding agents

- What the project is and promises, and the crate layout: [README.md](README.md)
- Disclosure policy — read before anything ships:
  [PUBLIC_DEVELOPMENT.md](PUBLIC_DEVELOPMENT.md)
- Issue, branch, commit and PR conventions:
  [CONTRIBUTING.md](CONTRIBUTING.md)
- Security reports: [SECURITY.md](SECURITY.md)
- Documentation lives in RustDoc only (`cargo doc`), with no `docs/`
  directory: concepts, design, flow diagrams, configuration and file formats,
  command usage and known limits are in the crate and module docs. Code
  documentation outranks stale issue text.
  [CONTRIBUTING.md](CONTRIBUTING.md#documentation) says what goes where.
- Layers: `retarget-core` holds `domain` (pure) and `app` (use cases and
  ports) and has no I/O dependency; the adapters are `crates/retarget/src/infra`.
  `arch-lint.toml` and `crates/retarget-core/tests/dependencies.rs` enforce it.
  A new adapter, store or harness goes in the umbrella crate, never in the core.
- The aim is the performance of the customer's agent or swarm (what a
  successful task costs, how long it takes, how often it succeeds).
  Statistics, rules, machine learning and AI are tools toward it, chosen by
  what they do for that performance, not by their rigour. A token saving the
  bill does not show is never kept.
- Two layers: the product finds a pattern, applies the catalogue's technique
  in a way that can be put back, and checks the fit against the limits the
  catalogue fixed; the Lab proves techniques with experiments. Heavy proof
  stays in the Lab. The crate doc of `retarget_core::domain` has the
  reasoning.
- The framework is open; data, a tuned catalogue and the features that find
  patterns are not in this repository and never go into its tree (not even
  ignored). They plug in as a catalogue value and as implementations of a
  port.
- The domain names no harness: attribute names, event shapes, prices and the
  catalogue of levers are the adapters'. Parts that need an AI's judgement are
  ports that propose; a verdict never goes through them.
- An adapter writes only files of the harness's project scope that can be put
  back, records what they held first, and does not put back a file a person has
  edited since. The user's own settings, permissions and credentials are shown
  to the user, never written.
- Telemetry captured from a real session carries account and session
  identifiers. Anonymise it before it becomes a test fixture
  ([PUBLIC_DEVELOPMENT.md](PUBLIC_DEVELOPMENT.md#block)).
- Never work on `main`; one branch per issue, cut from `origin/main` after a
  fetch. `just worktree-new <type> <slug>` from the main checkout does that and
  runs `just branch-check`.
- `just check` is green. [CONTRIBUTING.md](CONTRIBUTING.md#verification) says
  what it covers.
- Review a change locally before it goes up — by a person or an AI agent — and
  say in the PR body who or what reviewed it.
  [CONTRIBUTING.md](CONTRIBUTING.md#working-with-coding-agents--the-recommended-pattern)
  lists the questions a review answers.
