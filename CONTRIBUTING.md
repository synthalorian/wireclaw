# Contributing to ledger

Thanks for your interest in contributing! Here's how to get started.

## Development Setup

```bash
git clone https://github.com/synthalorian/ledger.git
cd ledger
cargo build
cargo test
```

## Code Standards

- **Formatting:** `cargo fmt` must produce no changes.
- **Linting:** `cargo clippy -- -D warnings` must pass with zero warnings.
- **Tests:** All tests must pass. New features should include tests.
- **Error handling:** Use `anyhow::Result` for application code. Never use `unwrap()` in non-test code.

## Pull Request Process

1. Fork the repository.
2. Create a feature branch from `main`.
3. Make your changes. Keep commits atomic and well-described.
4. Run `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test`.
5. Open a pull request against `main`.

## Commit Messages

- Use the imperative mood: "add feature" not "added feature".
- Keep the first line under 72 characters.
- Reference issues when applicable: "fix request body capture (#42)".

## Module Structure

| Module | Responsibility |
|--------|---------------|
| `cli.rs` | Clap argument definitions |
| `config.rs` | TOML config loading |
| `db.rs` | SQLite schema and queries |
| `proxy.rs` | HTTP proxy server |
| `logger.rs` | Request/response logging |
| `replay.rs` | Request replay engine |
| `search.rs` | Search and filtering |
| `export.rs` | HAR/curl/raw export |
| `tui.rs` | Interactive terminal UI |
| `models.rs` | Data structures |
| `error.rs` | Custom error types |

## Adding a New Feature

1. Define any new data structures in `models.rs`.
2. Add CLI arguments in `cli.rs` if the feature is user-facing.
3. Implement the logic in the appropriate module (or create a new one).
4. Wire it up in `main.rs`.
5. Add tests.

## Reporting Issues

- Check existing issues before opening a new one.
- Include the Rust version (`rustc --version`), OS, and steps to reproduce.
- For feature requests, describe the use case and expected behavior.

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
