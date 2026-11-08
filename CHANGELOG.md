# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- A lever is judged on the evidence its kind allows: a lever that only changes
  prices is kept provisionally on the cost terms its mechanism moves and then
  confirmed against a holdout; a lever that changes behaviour is judged on the
  cost per successful run directly. Runs are assigned to arms at random, and the
  stopping rule is fixed in advance.
- retarget may write any project-scoped file of the harness that it can put
  back (settings, agent definitions, project instructions), and does not put
  back a file a person has edited since.
