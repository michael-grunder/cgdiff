# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added AArch64-aware disassembly invocation and branch normalization.
- Added release binary builds for Linux x86-64 musl, Linux AArch64 musl, and
  macOS AArch64.
- Added `--filter-out` and TUI `!` filter-out support, which can be combined
  with normal filters.
- Added `--stdio --diff` output for unified diffs of listed functions'
  normalized disassembly.

### Documentation

- Added the initial `README.md` with project overview, requirements,
  installation, usage examples, TUI controls, scoring notes, normalization
  behavior, development commands, and license information.
- Added this initial `CHANGELOG.md` to track project changes.
