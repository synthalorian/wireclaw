# Ledger v1.0 Roadmap

## Current State
Core loop works: HTTP/HTTPS proxy capture → SQLite storage → list/search/replay/export/TUI.
53 tests passing. Release builds clean (6 dead_code warnings on stubbed modules).

---

## The 15 Gaps to v1.0

### P0 — Ship Blockers (do first)

**1. HTTPS MITM Interception** ✅
- Generate CA cert on first run (or `ledger ca generate`)
- Sign per-host certs dynamically
- Terminate TLS at proxy, re-encrypt to target
- Capture actual request/response bodies inside TLS (JSON payloads, headers)
- Store CA cert to `~/.local/share/ledger/ca.crt`
- Trust instructions for browsers/system
- **Why:** Without this, we're blind to 90% of API traffic

**2. Fix Replay Body Forwarding** ✅
- `build_http_request` has the body but verify hyper actually sends it
- Add test: replay a POST with JSON body, verify target receives it
- **Why:** Currently broken/unverified

**3. Request/Response Diff on Replay** ✅
- Compare original vs replayed response
- Show: status diff, header diffs, body diff (JSON-aware if possible)
- CLI flag: `--diff` on replay
- **Why:** Core value prop of replay is seeing what changed

---

### P1 — UX & Power Features

**4. TUI In-App Search/Filter** ✅
- `/` keybinding opens search prompt
- Filter by method, path, status, host
- Live filter-as-you-type
- **Why:** Scrolling 500 requests is unusable

**5. TUI JSON Syntax Highlighting** ✅
- Detect `content-type: application/json`
- Colorize keys, strings, numbers, booleans in detail pane
- Collapse/expand nested objects (optional)
- **Why:** Raw text dumps are hard to read

**6. Config File Generation (`ledger init`)** ✅
- `ledger init` creates `~/.config/ledger/config.toml` with defaults
- Include comments explaining each option
- **Why:** Zero-config is nice, but power users need knobs

**7. Request Editing Before Replay** ✅
- `ledger replay --id abc --edit` opens $EDITOR with request as JSON
- Modify headers, body, URL, method
- Save and replay modified version
- **Why:** Replaying identical requests is rarely useful

---

### P2 — Export & Integration

**8. Postman Collection Export** ✅
- `ledger export --format postman`
- Group by host as Postman "folders"
- Preserve headers, body, method
- Importable into Postman/Insomnia/Bruno
- **Why:** HAR is for browsers, Postman is for API devs

**9. Request Grouping in TUI** ✅
- Group by host (default) or by path prefix
- Expand/collapse groups
- Show group-level stats (count, avg latency, error rate)
- **Why:** Flat list doesn't scale

---

### P3 — Advanced Features

**10. Breakpoints / Intercept Mode** ✅
- `ledger capture --intercept`
- Pause on matching requests (by method, path, host)
- Show request, allow modify/reject/forward
- Resume or drop
- **Why:** Charles Proxy's killer feature

**11. Request Chaining / Variable Extraction** ✅
- `ledger replay --chain` — replay sequence of requests
- Extract values from response (JSONPath) into variables
- Substitute variables into subsequent requests
- Example: login → extract token → use token in GET /profile
- **Why:** Real workflows are multi-step

**12. Metrics & Stats**
- `ledger stats --session foo`
- Total requests, avg latency, error rate (4xx/5xx %)
- Top endpoints by hit count
- Latency histogram
- **Why:** Debugging performance issues

**13. WebSocket Support** 🟡
- Capture WebSocket frames (text, binary, ping/pong)
- Store as "exchanges" with direction (client→server, server→client)
- Replay WebSocket conversations
- **Why:** Modern APIs (GraphQL subscriptions, real-time data)

**14. Pre/Post Request Scripts** ✅
- Lua or JavaScript hooks
- `pre-request.lua`: modify request before sending
- `post-response.lua`: assert on response, extract data
- **Why:** Power users need automation

**15. Docker Image + CI/CD Integration**
- Dockerfile (multi-stage, distroless or alpine)
- GitHub Actions workflow to build/push to GHCR
- `docker run -p 8080:8080 -v ledger-data:/data ghcr.io/synthalorian/ledger`
- **Why:** Teams run this in CI to capture test traffic

---

## Suggested Session Order

| Session | Focus | Items | Status |
|---------|-------|-------|--------|
| 1 | MITM HTTPS | #1 | ✅ Done |
| 2 | Replay fixes + diff | #2, #3 | ✅ Done |
| 3 | TUI polish | #4, #5 | ✅ Done |
| 4 | Power tools | #6, #7 | ✅ Done |
| 5 | Export + grouping | #8, #9 | ✅ Done |
| 6 | Advanced | #10, #11, #12 | ✅ Done |
| 7 | WebSocket + scripts | #13, #14 | 🟡 #13 stubbed, #14 done |
| 8 | Packaging | #15, release | ✅ v0.2.0 tagged |

---

## Notes

- MITM is the only true blocker. Everything else is additive.
- For MITM: `rcgen` crate for cert generation, `rustls` for TLS termination
- WebSocket is lower priority — most APIs are still REST/GraphQL over HTTP
- Docker image enables CI use cases which is a different market segment

*This is the wave.* 🎹🦈
