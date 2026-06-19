#!/bin/bash
# Wireclaw Hackathon Demo Script
# Run this to generate a complete demo session with live traffic
# Perfect for screen recording a hackathon submission video

set -e

echo "🦞 Wireclaw Hackathon Demo"
echo "=========================="
echo ""

# Build release binary
echo "[1/6] Building release binary..."
cd "$(dirname "$0")"
cargo build --release 2>/dev/null

echo ""
echo "[2/6] Cleaning up old demo session..."
rm -f ~/.local/share/wireclaw/sessions/demo.db

echo ""
echo "[3/6] Starting capture proxy + dashboard..."
echo "        Proxy:  http://127.0.0.1:9090"
echo "        Dashboard: http://127.0.0.1:8746"
echo ""
./target/release/wireclaw capture \
    --session demo \
    --addr 127.0.0.1:9090 \
    --dashboard \
    --dashboard-addr 127.0.0.1:8746 &
PROXY_PID=$!

# Wait for services to start
sleep 2

echo "[4/6] Generating demo traffic..."

# Start a local test server
python3 -m http.server 8765 --bind 127.0.0.1 &
SERVER_PID=$!
sleep 1

export HTTP_PROXY=http://127.0.0.1:9090

# Make various API-like requests
echo "  → GET / (homepage)"
curl -s -o /dev/null http://127.0.0.1:8765/

echo "  → GET /api/users"
curl -s -o /dev/null "http://127.0.0.1:8765/?path=/api/users"

echo "  → POST /api/users (create user)"
curl -s -o /dev/null -X POST -H "Content-Type: application/json" \
    -d '{"name":"synth","email":"synth@example.com"}' \
    http://127.0.0.1:8765/

echo "  → GET /api/users/123"
curl -s -o /dev/null "http://127.0.0.1:8765/?path=/api/users/123"

echo "  → PUT /api/users/123 (update user)"
curl -s -o /dev/null -X PUT -H "Content-Type: application/json" \
    -d '{"name":"synthalorian"}' \
    http://127.0.0.1:8765/

echo "  → DELETE /api/users/123"
curl -s -o /dev/null -X DELETE http://127.0.0.1:8765/

echo "  → GET /api/products"
curl -s -o /dev/null "http://127.0.0.1:8765/?path=/api/products"

echo "  → POST /api/orders (create order)"
curl -s -o /dev/null -X POST -H "Content-Type: application/json" \
    -d '{"product_id":"456","quantity":2}' \
    http://127.0.0.1:8765/

echo ""
echo "[5/6] Showing captured traffic..."
./target/release/wireclaw list --session demo --limit 10

echo ""
echo "[6/6] Demo ready!"
echo ""
echo "  🌐 Open dashboard: http://127.0.0.1:8746"
echo "  🎨 Theme: Click 🌆 for Synthwave '84 (default)"
echo "  🌙 Theme: Click 🌙 for Dark mode"
echo "  ☀️ Theme: Click ☀️ for Light mode"
echo ""
echo "  Try these commands:"
echo "    wireclaw stats --session demo"
echo "    wireclaw diff --a <id1> --b <id2> --session demo"
echo "    wireclaw openapi --session demo --output demo-api.json"
echo ""
echo "  Press Ctrl+C to stop the demo."
echo ""

# Cleanup on exit
cleanup() {
    echo ""
    echo "🦞 Cleaning up..."
    kill $PROXY_PID 2>/dev/null || true
    kill $SERVER_PID 2>/dev/null || true
    exit 0
}
trap cleanup INT TERM

# Keep running until interrupted
wait $PROXY_PID
