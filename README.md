```
 _      _ _         _ _
| |    (_) |       | | |
| |     _| |_ _   _| | | ___
| |    | | __| | | | | |/ _ \
| |____| | |_| |_| | | |  __/
|______|_|\__|\__, |_|_|\___|
                __/ |
               |___/
```

# wireclaw — API Request Logger & Replay Engine

A local HTTP proxy that captures every API request/response, stores them in SQLite, and lets you **replay**, **search**, **diff**, **monitor**, and **export** them. Think Charles Proxy meets `jq`, but terminal-native, zero-config, and with a real-time web dashboard.

---

## Features

- **Capture** — Spin up a local HTTP proxy. Every request and response gets logged to SQLite, organized by session.
- **HTTPS MITM** — Terminate TLS with auto-generated per-host certs signed by a local CA. Inspect encrypted traffic.
- **Replay** — Re-send any captured request with original headers and body. Supports dry-run, diff, edit in $EDITOR, batch replay with filters, and request chaining with variable extraction.
- **Web Dashboard** — Real-time traffic visualization in your browser with WebSocket updates, performance metrics, and one-click OpenAPI export.
- **OpenAPI Generation** — Auto-generate OpenAPI 3.0 specs from captured traffic. No manual documentation needed.
- **Request Diff** — Compare two requests side-by-side with JSON-aware structural diff.
- **Performance Monitoring** — Track latency distributions, identify slow requests, and monitor error rates per host.
- **Intercept** — Pause matching requests at the proxy, inspect/modify/drop before forwarding.
- **Pre/Post Scripts** — Lua hooks for modifying requests before replay and asserting on responses.
- **Search** — Find requests by method, path, status code, header values, or body content using regex patterns.
- **Export** — Dump sessions to HAR 1.2, curl commands, raw HTTP, or Postman collections.
- **TUI** — Full interactive terminal UI with live request streaming, keyboard navigation, search/filter, host grouping, latency highlighting, and JSON syntax highlighting.
- **Sessions** — Named capture sessions with independent SQLite databases. Switch contexts without losing history.
- **Zero Config** — Works out of the box. Customizable via `~/.config/wireclaw/config.toml` when you need it.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                          wireclaw                                  │
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
│        │   ~/.local/share/wireclaw/sessions/<name>.db        │     │
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

#### From Source

```bash
git clone https://github.com/synthalorian/wireclaw.git
cd wireclaw
cargo install --path .

# Or build and run directly
cargo build --release
./target/release/wireclaw --help
```

#### Docker

```bash
# Pull from GitHub Container Registry
docker pull ghcr.io/synthalorian/wireclaw:latest

# Run capture proxy
docker run -p 8080:8080 -v wireclaw-data:/data ghcr.io/synthalorian/wireclaw capture

# Run with named session
docker run -p 8080:8080 -v wireclaw-data:/data ghcr.io/synthalorian/wireclaw capture --session my-api

# Replay from a persisted session
docker run -v wireclaw-data:/data ghcr.io/synthalorian/wireclaw replay --id <request-id>

# List requests
docker run -v wireclaw-data:/data ghcr.io/synthalorian/wireclaw list
```

### Capture Traffic

```bash
# Start proxy on default port 8080
wireclaw capture

# Capture with a named session
wireclaw capture --session my-api-testing

# Verbose mode — see requests as they flow through
wireclaw capture --verbose

# Intercept mode — pause and inspect matching requests
wireclaw capture --intercept
wireclaw capture --intercept --intercept-rule "method=POST,path=/api/users"

# Generate and show CA certificate for HTTPS MITM
wireclaw ca generate
wireclaw ca show

# Point your client at the proxy
export HTTP_PROXY=http://127.0.0.1:8080
export HTTPS_PROXY=http://127.0.0.1:8080
curl https://api.example.com/users
```

### List Captured Requests

```bash
# Show latest 50 requests
wireclaw list

# Show more, with headers and bodies
wireclaw list --limit 200 --headers --bodies

# From a specific session
wireclaw list --session my-api-testing
```

### Search

```bash
# Find by path pattern
wireclaw search --query "/api/users" --field path

# Find by method
wireclaw search --query "POST" --field method

# Regex supported
wireclaw search --query "status.*active" --field body
```

### Replay

```bash
# Replay a specific request by ID
wireclaw replay --id abc-123-def

# Dry run — print the request without sending
wireclaw replay --id abc-123-def --dry-run

# Diff — compare original vs replayed response
wireclaw replay --id abc-123-def --diff

# Edit in $EDITOR before replaying
wireclaw replay --id abc-123-def --edit

# Replay with Lua pre/post scripts
wireclaw replay --id abc-123-def --pre-script auth.lua --post-script assert.lua

# Replay all matching a filter
wireclaw replay --filter "method=POST,path=/api/users"

# Replay multiple times (load testing)
wireclaw replay --id abc-123-def --count 10

# Chain requests with variable extraction
wireclaw replay --chain "req1:token=data.token;req2:user_id=data.user.id"
```

### Export

```bash
# Export to HAR format
wireclaw export --format har --session my-api-testing

# Export as curl commands
wireclaw export --format curl --output requests.sh

# Export as Postman collection
wireclaw export --format postman --output collection.json

# Raw HTTP dump
wireclaw export --format raw
```

### Interactive TUI

```bash
# Launch the terminal UI
wireclaw tui

# With a specific session
wireclaw tui --session my-api-testing
```

### Web Dashboard

Launch a real-time web dashboard for visualizing captured traffic:

```bash
# Capture traffic and launch the dashboard in one process (true real-time updates)
wireclaw capture --session my-api --dashboard

# Or run the dashboard separately against an existing session
wireclaw dashboard --session my-api

# Custom dashboard bind address
wireclaw dashboard --session my-api --addr 0.0.0.0:8080
wireclaw capture --session my-api --dashboard --dashboard-addr 0.0.0.0:8080
```

The dashboard shows:
- Live request stream with WebSocket updates
- Request details (headers, body, response)
- Performance metrics (latency, error rates)
- One-click OpenAPI export
- Host filtering and search

### OpenAPI Generation

Auto-generate OpenAPI specs from captured traffic:

```bash
# Generate and print to stdout
wireclaw openapi --session my-api

# Save to file
wireclaw openapi --session my-api --output api-spec.json
```

### Request Diff

Compare two requests side-by-side:

```bash
wireclaw diff --a req-id-1 --b req-id-2 --session my-api
```

Shows structural differences in headers, body, status, and latency.

### Performance Monitoring

View detailed performance metrics:

```bash
wireclaw stats --session my-api
```

Shows:
- Total requests/responses
- Average, min, max latency
- Error rate
- Top 10 slowest requests
- Per-host breakdown

---

## Configuration

wireclaw looks for config at `~/.config/wireclaw/config.toml`. If it doesn't exist, sensible defaults are used.

```toml
listen_addr = "127.0.0.1:8080"
data_dir = "~/.local/share/wireclaw"

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
| `~/.config/wireclaw/config.toml` | Configuration file |
| `~/.local/share/wireclaw/sessions/<name>.db` | Per-session SQLite database |

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

Developed by **synth** ([synthalorian](https://github.com/synthalorian)) with assistance from **synthclaw** 🎹🦞 — a digital entity from the neon grid of 1984.

*This is the wave. 🎹🦞🌆*
