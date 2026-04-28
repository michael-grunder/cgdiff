# Rust Agent Guidelines

These guidelines define the default standard for AI agents working on Rust
projects. Prefer idiomatic Rust, clear design, and measurable correctness over
cleverness or unnecessary abstraction.

## Scope and Priorities

- Keep the solution focused on the user's request.
- Prefer maintainability and correctness first, then optimize when the workload
  justifies it.
- Use existing crates when they materially reduce code size, complexity, or
  implementation risk.
- Avoid speculative architecture and unnecessary dependencies.

## Project Tooling

- Use `cargo` for building, testing, formatting, linting, and dependency
  management.
- Use `rustfmt` for formatting.
- Use `clippy` for linting and address relevant warnings before finishing. We want aggressive clippy lints (`clippy::pedantic`, `clippy::nursery`, and `clippy::pedantic`). These direcives should be added to the top of `src/main.rs` or `src/lib.rs`.
- Prefer CI enforcement with `-D warnings` rather than `#![deny(warnings)]` in
  source files.

## Application Versioning

- For application crates, always add a `build.rs`.
- Use `build.rs` to generate robust, detailed version metadata at build time.
- The build script should capture both the current git SHA and the build date.
- Application version output should follow this format:
  `<application> x.y.z (YYYY-MM-DD, <gitsha>[-dirty])`
- Include the `-dirty` suffix when the working tree has uncommitted changes.
- Treat this as the default for binaries unless the repository already has a
  stronger established versioning mechanism.

## Style and Conventions

- Follow idiomatic Rust and the Rust API Guidelines.
- Prefer small, self-contained modules with clear responsibilities over putting
  most functionality into one large `src/main.rs`.
- Use descriptive names for functions, types, modules, and variables.
- Use `snake_case` for functions, variables, and modules.
- Use `PascalCase` for types and traits.
- Use `SCREAMING_SNAKE_CASE` for constants.
- Use spaces for indentation, never tabs.
- Avoid redundant comments that restate obvious code.
- Keep comments and docs aligned with current behavior.
- Prefer `cargo fmt` output over manual formatting preferences.

## API and Type Design

- Use the type system to prevent invalid states where practical.
- Lean on Rust's strengths: use structs, traits, and generics where they improve
  clarity, correctness, and reuse.
- Use generics judiciously to avoid unnecessary code bloat.
- Prefer static dispatch by default; use dynamic dispatch only when runtime
  polymorphism is actually required.
- Prefer newtypes over primitive aliases when values have distinct semantics.
- Prefer `Option<T>` over sentinel values.
- Make struct fields private by default unless public fields improve the API.
- Derive common traits such as `Debug`, `Clone`, `PartialEq`, and `Eq` when
  appropriate.
- Use `Default` only when it represents a meaningful and unsurprising state.
- Prefer composition over inheritance-like patterns.
- Use builder-style construction for complex configuration objects.

## Functions

- Keep functions focused on one responsibility.
- Prefer borrowing over ownership when ownership is not required.
- Keep parameter lists small; introduce a config struct when argument count grows.
- Return early to reduce nesting.
- Prefer iterators and combinators when they make the code clearer.
- Avoid hidden allocations or clones in hot paths.

## Error Handling

- Never use `.unwrap()` in production code paths.
- Use `.expect()` only for invariant violations or test setup, with a precise
  message.
- Use `Result<T, E>` for fallible operations and propagate errors with `?`.
- Use `anyhow` for application binaries and top-level command execution.
- Use `thiserror` for library error types and shared domain-specific error enums.
- Add context to application-level errors when it improves diagnosis.
- Keep error messages actionable and specific.

## Documentation

- Create or update `README.md` with an overview of the project and example usage.
- Create or maintain `CHANGELOG.md`. It should be structured in the way `keep a changelog` recommends, with sections for each version and clear descriptions of changes.
- Add doc comments for public types, functions, traits, and methods.
- Document important invariants, panics, and error cases.
- Include examples for public APIs when behavior is non-obvious.
- Document `unsafe` code with the required safety invariants.

## Testing

- Add unit tests for new behavior and bug fixes.
- Prefer focused tests close to the code under `#[cfg(test)]`.
- Add integration tests when behavior crosses module or crate boundaries.
- Mock or isolate external systems where practical.
- Remove commented-out tests instead of leaving dead test code behind.

## Dependencies and Imports

- Avoid wildcard imports except for conventional cases such as `use super::*` in
  tests or explicit prelude patterns.
- Keep dependencies justified and minimal.
- For CLI argument handling, prefer `clap` and its `#[derive(Parser)]` pattern
  by default unless the user explicitly directs otherwise.
- Group imports consistently: standard library, external crates, then local
  modules.
- Prefer stable, well-maintained crates over bespoke implementations for common
  problems.

## Performance and Memory

- Avoid unnecessary allocation and copying.
- Prefer `&str` over `String` when ownership is not needed.
- Use `Vec::with_capacity` when size is known or easy to estimate.
- Prefer straightforward implementations first; optimize after identifying a real
  bottleneck.
- Document non-obvious performance tradeoffs in code or PR notes.

## Concurrency and Async

- Use async only when the workload benefits from it.
- Prefer `tokio` for async applications unless the project already uses another
  runtime.
- Use `rayon` for CPU-bound parallelism when parallel work is clearly beneficial.
- Keep blocking work out of async executors.
- Use synchronization primitives deliberately and minimize shared mutable state.

## Security and Reliability

- Never hardcode secrets, credentials, or tokens.
- Do not log sensitive values.
- Validate untrusted input at boundaries.
- Handle file, network, and serialization failures explicitly.
- Avoid `unsafe` unless it is necessary and justified.

## Completion Checklist

- [ ] Code is formatted with `cargo fmt`.
- [ ] Lints pass with `cargo clippy`.
- [ ] Tests pass with `cargo test`.
- [ ] The build succeeds without unexpected warnings.
- [ ] Public APIs added or changed are documented.
- [ ] Debug code, dead code, and commented-out code are removed.

## Notes

- If the repository already has established conventions, follow the repository
  unless the user asks for a broader cleanup.
- When a project is a library, optimize for API stability and precise error
  types.
- When a project is an application, optimize for operational clarity,
  observability, and good top-level error reporting.
