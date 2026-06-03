#!/bin/bash
set -e

# Clean up
rm -f /home/synth/.local/share/ledger/sessions/https_test.db /tmp/ledger_https.log

# Start proxy in background, capture PID
/home/synth/projects/ledger/target/release/ledger capture --session https_test --verbose --addr 127.0.0.1:9090 > /tmp/ledger_https.log 2>&1 &
PROXY_PID=$!
echo "Proxy PID: $PROXY_PID"

# Wait for it to bind
sleep 2

# Check it's running
if ! kill -0 $PROXY_PID 2>/dev/null; then
    echo "Proxy died immediately"
    cat /tmp/ledger_cap.log
    exit 1
fi

# Send a request through it to an HTTPS endpoint
echo "Sending HTTPS request..."
curl -s -x http://127.0.0.1:9090 https://httpbin.org/get 2>&1 | head -5 || true

sleep 1

# Kill proxy
kill $PROXY_PID 2>/dev/null || true
wait $PROXY_PID 2>/dev/null || true

# Show logs
echo "=== Proxy logs ==="
cat /tmp/ledger_https.log

# Show DB contents
echo "=== DB contents ==="
/home/synth/projects/ledger/target/release/ledger list --session https_test --limit 5
