# Contributing

Thanks for contributing to Herdr Organizations. Keep changes focused, preserve existing project data and lifecycle behavior, and include tests for node hierarchy, context scoping or profile changes.

## Development setup

- Rust and Cargo 1.89 or newer
- Herdr 0.9.1 or newer for client checks
- Git
- Optional agent CLIs for manual profile checks

Build and run the local suite:

```sh
cargo build --locked
cargo test --locked
```

Before submitting, run the same gates as CI:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

Use fake Runner scenarios for Herdr, GitHub CLI and SSH behavior. Use a disposable Herdr session for the popup, pane focus, live agent startup, remote machines and mouse-forwarding checks. See [Manual validation](docs/manual-test.md).

## Design constraints

- Keep project paths and legacy `Thread` records compatible. Do not add migrations for fields with safe serde defaults.
- Validate an entire hierarchy before changing node state. Hold the existing project lock for record creation and rollback.
- Compose node context from root through parent to target. Never include sibling or descendant scopes.
- Pass harness configuration as argv components. Do not build shell command strings from profile fields.
- Keep Herdr calls behind the existing Runner and `Herdr` interfaces. Do not add MCP tools, a daemon, a database or harness-specific registered tools.
- Do not treat `can_spawn` or agent permission profiles as a security boundary. Explain actual limits in code and docs.
- Keep the Herdr Organizations plugin command and popup statically declared. The recursive tree belongs in the popup process.

## Documentation and licensing

Update the README and relevant operations, architecture, getting-started or manual test documentation when behavior changes. Preserve the upstream MIT attribution in `LICENSE` and `NOTICE`. Avoid personal paths, credentials, generated state, and em dash punctuation in tracked changes.
