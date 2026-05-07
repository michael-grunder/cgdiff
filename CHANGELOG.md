# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added AArch64-aware disassembly invocation and branch normalization.
- Added release binary builds for Linux x86-64 musl, Linux AArch64 musl, and
  macOS AArch64.
- Added `--exclude` and TUI `!` exclude support, which can be combined with
  `--include` and TUI `/` includes.
- Added `--include-unique` and `--include-identical` aliases for showing
  hidden function rows.
- Added `--stdio --diff` output for unified diffs of listed functions'
  normalized disassembly.
- Added a built-in syntax-highlighted TUI diff viewer for selected functions,
  while preserving external diff editor handoff with `e`.
- Added side-by-side layout mode for the built-in TUI diff viewer.
- Added folding for long unchanged side-by-side diff regions, configurable
  with `diff_context` or `--diff-context`.
- Added configurable syntax highlighting themes, per-token color overrides,
  and `--list-themes` previews.

### Documentation

- Added the initial `README.md` with project overview, requirements,
  installation, usage examples, TUI controls, scoring notes, normalization
  behavior, development commands, and license information.
- Added this initial `CHANGELOG.md` to track project changes.
