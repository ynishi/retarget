# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- The domain of the core: runs as trees of agent nodes, cost terms and
  mechanisms, levers, the Lab's experiments and verdicts, and a ledger that
  writes and puts back a lever's place only where no person edited it.
- The product layer: a catalogue of techniques, findings from detectors,
  adoptions rolled out from shown to a share of runs to all but a holdout,
  and a fit check that puts back a technique shown to make a customer's runs
  worse. Detector and CatalogueSource ports let tuned ones plug in.
- Claude Code's telemetry events (OTLP/JSON logs) become runs: each session's
  tree of the main conversation, its subagents and compaction, with every
  request's tokens, reported cost and, from a price table the caller passes,
  its cost by kind of token. What cannot be read is noted, never guessed.

### Changed

- The aim is the performance of the customer's agent or swarm. The product
  finds a pattern of waste, applies a known technique from a catalogue and
  checks that it fits; techniques are proven in a Lab. The framework is open;
  data, a tuned catalogue and pattern features plug in from outside.
- In the Lab, a lever is judged on the evidence its kind allows: a lever that only changes
  prices is kept provisionally on the cost terms its mechanism moves and then
  confirmed against a holdout; a lever that changes behaviour is judged on the
  cost per successful run directly. Runs are assigned to arms at random, and the
  stopping rule is fixed in advance.
- retarget may write any project-scoped file of the harness that it can put
  back (settings, agent definitions, project instructions), and does not put
  back a file a person has edited since.
