```
 _   _   _   _   _   _   _   _  
/ \ / \ / \ / \ / \ / \ / \ / \ 
( w | i | r | e | c | l | a | w )
 \_/ \_/ \_/ \_/ \_/ \_/ \_/ \_/
```

# wireclaw — API Traffic Observability Platform

> **Auto-document your API by watching it work.**

Wireclaw is a local HTTP/HTTPS proxy that captures every API request and response, stores them in SQLite, and gives you a real-time web dashboard, terminal UI, and one-click OpenAPI export. No code changes. No SDKs. No manual documentation.

**Built with Rust. Zero `unsafe`. Zero config.**

---

## Why Wireclaw?

| The Problem | The Cost |
|-------------|----------|
| API docs drift from code the moment they ship | Hours of manual updates, outdated contracts |
| Debugging production issues means hunting through logs | Slower incident response, frustrated teams |
| Onboarding new devs requires explaining API behavior | Repeated knowledge transfer, tribal knowledge |
| No easy way to compare "this request works, that one doesn't" | Staring at JSON diffs in text editors |

**Wireclaw turns API observability from a chore into a byproduct of normal development.**

Point your HTTP client at the proxy. Ship your code. Browse the dashboard. Export the spec. Done.

---

## Features

- **🔴 Capture** — Local HTTP/HTTPS proxy. Every request/response logged to SQLite, organized by named session.
- **🔒 HTTPS MITM** — Auto-generated per-host TLS certificates. Inspect encrypted traffic without touching client code.
- **📊 Real-Time Dashboard** — WebSocket-powered traffic visualization. One-click OpenAPI export. Three themes including Synthwave '84.
- **📋 OpenAPI Auto-Generation** — Generate OpenAPI 3.0 specs from live traffic. Real examples, inferred schemas, no manual work.
- **🔁 Replay & Chain** — Re-send any captured request. Dry-run, diff, edit in `$EDITOR`, batch replay, and chain requests with Lua variable extraction.
- **🔍 Search & Diff** — Regex search across method, path, headers, body. JSON-aware structural diff between any two requests.
- **📈 Performance Monitoring** — Latency percentiles (p50, p95, p99), error rates, slow request detection.
- **🖥️ Terminal UI** — Full ratatui interface with live streaming, keyboard navigation, JSON syntax highlighting. Works over SSH.
- **📤 Export** — HAR 1.2, curl commands, raw HTTP, Postman collections.
- **⚡ Zero Config** — Works out of the box. Customizable via `~/.config/wireclaw/config.toml` when you need it.

---

## Quick Start

### Install from Source

```bash
git clone https://github.com/synthalorian/wireclaw.git
cd wireclaw
cargo install --path .
```

### Capture Traffic

```bash
# Start proxy + dashboard
wireclaw capture --session my-api --dashboard

# Point your client at the proxy
export HTTP_PROXY=http://127.0.0.1:8080
curl https://api.example.com/users

# Open the dashboard
# → http://localhost:8746
```

### Generate OpenAPI from Live Traffic

```bash
# After capturing traffic, export the spec
wireclaw openapi --session my-api --output api-spec.json
```

### Replay & Diff

```bash
# List captured requests
wireclaw list --session my-api

# Replay a specific request
wireclaw replay --id <request-id>

# Compare two requests side-by-side
wireclaw diff --a <id1> --b <id2> --session my-api
```

### Launch the TUI

```bash
wireclaw tui --session my-api
```

---

## Demo

```bash
# Full demo: capture + dashboard + sample traffic
./demo.sh
```

The demo script starts a proxy, generates sample API traffic, and opens the dashboard. Perfect for screen recording a submission video.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        wireclaw                              │
│                                                              │
│  ┌────────┐  ┌────────┐  ┌────────┐  ┌────────────────┐  │
│  │  CLI   │  │  TUI   │  │ Config │  │  Web Dashboard │  │
│  │(clap)  │  │(ratatui│  │(TOML)  │  │   (axum+ws)   │  │
│  └───┬────┘  └───┬────┘  └───┬────┘  └───────┬────────┘  │
│      │           │           │               │             │
│  ┌───▼───────────▼───────────▼───────────────▼─────────┐  │
│  │                   Core Dispatch                        │  │
│  └──┬──────┬────────┬─────────┬──────────┬────────────┘  │
│     │      │        │         │          │                │
│  ┌──▼──┐ ┌──▼──┐ ┌──▼───┐ ┌──▼──┐  ┌───▼────┐           │
│  │Proxy│ │Replay│ │Search│ │Export│  │ Logger │           │
│  │(hyper│ │      │ │(regex)│ │(HAR) │  │        │           │
│  └──┬──┘ └──────┘ └──────┘ └──────┘  └───┬────┘           │
│     │                                      │                │
│     │  ┌───────────────────────────────────▼──────────┐   │
│     └──│          SQLite Storage (sqlx)               │   │
│        │   sessions.db → requests → responses        │   │
│        │   ~/.local/share/wireclaw/sessions/*.db       │   │
│        └───────────────────────────────────────────────┘   │
│                                                              │
│  Data Flow: Client → Proxy → Target → Proxy → Client        │
│                       ↓                                      │
│                   Logger → SQLite                              │
└─────────────────────────────────────────────────────────────┘
```

---

## Technical Highlights

- **34,000+ lines of Rust** — zero `unsafe` blocks
- **53 unit tests** — all passing
- **SQLite + sqlx** — type-safe async database operations
- **HTTPS MITM** — auto-generated per-host certificates via `rcgen`
- **Lua scripting** — hooks for request/response transformation
- **WebSocket proxy** — captures and replays WebSocket frames
- **HAR/Postman/curl export** — industry-standard formats

---

## Configuration

wireclaw looks for config at `~/.config/wireclaw/config.toml`. Sensible defaults are used if it doesn't exist.

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

Each session gets its own SQLite database with indexed tables for requests, responses, and session metadata.

---

## Development

```bash
# Build
cargo build

# Test
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

Developed by **synth 🎹🤺** ([synthalorian](https://github.com/synthalorian)) with assistance from **synthclaw** 🎹🦞 — a digital entity from the neon grid of 1984.

*This is the wave. 🎹🦞🌆*

---

## ☕ Support the Developer

If this project saved you time, solved a problem, or just made your day a little more neon, you can fuel the next one:

[![Buy Me A Coffee](https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png)](https://buymeacoffee.com/synthalorian)
