```
 ██╗     ███████╗██████╗ ████████╗██╗  ██╗███████╗
 ██║     ██╔════╝██╔══██╗╚══██╔══╝██║  ██║██╔════╝
 ██║     █████╗  ██████╔╝   ██║   ███████║███████╗
 ██║     ██╔══╝  ██╔══██╗   ██║   ██╔══██║╚════██║
 ███████╗███████╗██║  ██║   ██║   ██║  ██║███████║
 ╚══════╝╚══════╝╚═╝  ╚═╝   ╚═╝   ╚═╝  ╚═╝╚══════╝
```

# ledger — API Request Logger & Replay Engine

A local HTTP proxy that captures every API request/response, stores them in SQLite, and lets you **replay**, **search**, and **export** them. Think Charles Proxy meets `jq`, but terminal-native and zero-config.

---

## Features

- **Capture** — Spin up a local HTTP proxy. Every request and response gets logged to SQLite, organized by session.
- **Replay** — Re-send any captured request with original headers and body. Supports dry-run mode and batch replay with filters.
- **Search** — Find requests by method, path, status code, header values, or body content using regex patterns.
- **Export** — Dump sessions to HAR 1.2 (for browser DevTools), curl commands (for scripts), or raw HTTP (for debugging).
- **TUI** — Full interactive terminal UI with live request streaming, keyboard navigation, and syntax-highlighted detail view.
- **Sessions** — Named capture sessions with independent SQLite databases. Switch contexts without losing history.
- **Zero Config** — Works out of the box. Customizable via `~/.config/ledger/config.toml` when you need it.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                          ledger                                  │
│                                                                  │
│  ┌──────────┐    ┌──────────┐    ┌──────────┐    ┌──────────┐  │
│  │   CLI     │    │   TUI    │    │  Config  │    │  Error   │  │
│  │ (clap)    │    │(ratatui) │    │  (TOML)  │    │ (anyhow) │  │
│  └────┬─────┘    └────┬─────┘    └────┬─────┘    └──────────┘  │
│       │               │               │                         │
│  ┌────▼───────────────▼───────────────▼──────────────────────┐  │
│  │                     Core Dispatch                          │  │
│  └──┬────────┬─────────┬──────────┬──────────┬───────────────┘  │
│     │        │         │          │          │                   │
│  ┌──▼──┐  ┌──▼──┐  ┌──▼───┐  ┌──▼──┐  ┌──▼──────┐            │
│  │Proxy│  │Replay│  │Search│  │Export│  │ Logger  │            │
│  │(hyper) │      │  │(regex)│  │(HAR)│  │         │            │
│  └──┬──┘  └──────┘  └──────┘  └──────┘  └──┬──────┘            │
│     │                                         │                  │
│     │  ┌──────────────────────────────────────▼───────────┐     │
│     └──│              SQLite Storage (sqlx)               │     │
│        │   sessions.db → requests → responses              │     │
│        │   ~/.local/share/ledger/sessions/<name>.db        │     │
│        └──────────────────────────────────────────────────┘     │
│                                                                  │
│  Data Flow: Client → Proxy → Target → Proxy → Client            │
│                       ↓                                      │
│                   Logger → SQLite                               │
└─────────────────────────────────────────────────────────────────┘
```

---

## Quick Start

### Install

```bash
# From source
git clone https://github.com/synthalorian/ledger.git
cd ledger
cargo install --path .

# Or build and run directly
cargo build --release
./target/release/ledger --help
```

### Capture Traffic

```bash
# Start proxy on default port 8080
ledger capture

# Capture with a named session
ledger capture --session my-api-testing

# Verbose mode — see requests as they flow through
ledger capture --verbose

# Point your client at the proxy
export HTTP_PROXY=http://127.0.0.1:8080
curl https://api.example.com/users
```

### List Captured Requests

```bash
# Show latest 50 requests
ledger list

# Show more, with headers and bodies
ledger list --limit 200 --headers --bodies

# From a specific session
ledger list --session my-api-testing
```

### Search

```bash
# Find by path pattern
ledger search --query "/api/users" --field path

# Find by method
ledger search --query "POST" --field method

# Regex supported
ledger search --query "status.*active" --field body
```

### Replay

```bash
# Replay a specific request by ID
ledger replay --id abc-123-def

# Dry run — print the request without sending
ledger replay --id abc-123-def --dry-run

# Replay all matching a filter
ledger replay --filter "method=POST,path=/api/users"

# Replay multiple times (load testing)
ledger replay --id abc-123-def --count 10
```

### Export

```bash
# Export to HAR format
ledger export --format har --session my-api-testing

# Export as curl commands
ledger export --format curl --output requests.sh

# Raw HTTP dump
ledger export --format raw
```

### Interactive TUI

```bash
# Launch the terminal UI
ledger tui

# With a specific session
ledger tui --session my-api-testing
```

---

## Configuration

ledger looks for config at `~/.config/ledger/config.toml`. If it doesn't exist, sensible defaults are used.

```toml
listen_addr = "127.0.0.1:8080"
data_dir = "~/.local/share/ledger"

[session]
auto_create = true
default_name = "default"

[proxy]
listen_addr = "127.0.0.1:8080"
timeout_secs = 30
max_body_size = 10485760  # 10MB
capture_headers = true
capture_bodies = true

[replay]
delay_ms = 0
follow_redirects = true
max_redirects = 10
```

---

## Data Storage

| Path | Purpose |
|------|---------|
| `~/.config/ledger/config.toml` | Configuration file |
| `~/.local/share/ledger/sessions/<name>.db` | Per-session SQLite database |

Each session gets its own SQLite database. The schema includes indexed tables for requests, responses, and session metadata.

---

## Development

```bash
# Build
cargo build

# Check without building
cargo check

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt
```

---

## License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

---

## Credits

Developed by **synth** ([synthalorian](https://github.com/synthalorian)) with assistance from **synthshark** 🎹🦈 — a digital entity from the neon grid of 1984.

*This is the wave. 🎹🦈🌆*
