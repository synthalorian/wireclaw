# Wireclaw — Hermes Agent Accelerated Business Hackathon Submission

## 🦞 The Problem

API documentation is always out of date. Developers spend hours writing OpenAPI specs by hand, only for them to drift from reality the moment code ships. Meanwhile, debugging API issues means digging through logs, Postman collections, and praying someone saved the curl command.

## ✨ The Solution

**Wireclaw** — an API traffic observability platform that auto-documents your API by watching it work. No code changes. No SDKs. Just point your HTTP client at the proxy and watch the magic happen.

## 🎯 Key Features

### 1. Real-Time Web Dashboard
- Live traffic visualization with WebSocket updates
- **Three built-in themes**: Synthwave '84 (default), Dark, and Light
- One-click OpenAPI export from the dashboard
- Performance metrics at a glance
- Theme switcher with localStorage persistence

### 2. OpenAPI Auto-Generation
- Captures real HTTP traffic and generates OpenAPI 3.0.3 specs
- Infers path parameters, query strings, headers, and schemas
- Includes real request/response examples from live traffic
- Export as JSON for import into Swagger UI, Postman, or code generators

### 3. Request Diff
- Compare any two captured requests side-by-side
- JSON-aware structural diff (not just text comparison)
- Highlights added/removed/changed fields in headers and body
- Perfect for debugging "why does this request work but that one doesn't?"

### 4. Performance Monitoring
- Latency tracking with percentiles (p50, p95, p99)
- Per-host breakdown of error rates and response times
- Automatic slow request detection and alerting
- TUI shows latency in color (green → yellow → red)

### 5. Terminal-Native TUI
- Full interactive terminal interface with ratatui
- Live request streaming with keyboard navigation
- Host grouping, search/filter, JSON syntax highlighting
- Works over SSH — no GUI required

### 6. Theme System 🎨
- **Synthwave '84**: Deep purple (#0a0014) background, electric purple borders, cyan (#00f0ff) accents, magenta (#ff006e) highlights, yellow (#ffbe0b) warnings — neon glow effects on badges and buttons
- **Dark**: GitHub-style dark mode with blue accents
- **Light**: Clean light mode for daytime use
- Theme preference persisted in browser localStorage

## 🏗️ Technical Highlights

- **34,000+ lines of Rust** — zero `unsafe` blocks
- **53 unit tests** — all passing
- **SQLite + sqlx** — type-safe async database operations
- **HTTPS MITM** — auto-generated per-host certificates via `rcgen`
- **Lua scripting** — hooks for request/response transformation
- **WebSocket proxy** — captures and replays WebSocket frames
- **HAR/Postman/curl export** — industry-standard formats

## 📊 Business Impact

| Pain Point | Wireclaw Solution |
|-----------|---------------------|
| API docs out of date | Auto-generate from live traffic |
| Debugging production issues | Capture and replay exact requests |
| Onboarding new developers | Browseable API dashboard |
| Performance regressions | Latency tracking and alerts |
| API contract drift | Diff any two request versions |

## 🚀 3-Minute Demo

```bash
# One terminal: capture + live dashboard (real-time WebSocket updates)
wireclaw capture --session demo --dashboard

# Or run them separately:
#   wireclaw capture --session demo
#   wireclaw dashboard --session demo

# Make some API calls
export HTTP_PROXY=http://localhost:8080
curl https://api.github.com/users/synthalorian
curl https://api.github.com/repos/synthalorian/wireclaw

# Browser: http://localhost:8746
# → Watch real-time traffic flowing in
# → Click "Export OpenAPI" to download the spec
# → Toggle themes with 🌆 / 🌙 / ☀️ buttons

# Compare two requests
wireclaw list --session demo  # get request IDs
wireclaw diff --a <id1> --b <id2> --session demo

# Check performance
wireclaw stats --session demo
```

## 🛠️ Built With

- Rust 1.85+
- Tokio (async runtime)
- Hyper (HTTP proxy)
- Axum (web dashboard)
- Ratatui (terminal UI)
- SQLite + sqlx (storage)
- MLua (scripting hooks)
- rcgen (TLS certificate generation)

## 📦 Installation

```bash
cargo install --path .
# or
cargo build --release
./target/release/wireclaw --help
```

## 📝 Hackathon Theme Alignment

**"Improve workflow in the working industry"**

Wireclaw eliminates the friction between writing code and documenting APIs. It turns API observability from a chore into a byproduct of normal development. Teams using Wireclaw ship faster, debug quicker, and never write an outdated API doc again.

---

**Developed by:** synth (synthalorian)  
**With assistance from:** synthclaw 🎹🦞 — a digital entity from the neon grid of 1984

*This is the wave. 🌆*
