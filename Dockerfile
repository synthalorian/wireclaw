# Ledger — Local HTTP proxy for API traffic capture, replay, and inspection
# Multi-stage build: compile in Rust builder, copy binary to distroless runtime

# ── Stage 1: Build ───────────────────────────────────────────────────────────
FROM rust:1.88-alpine AS builder

# Install build dependencies
RUN apk add --no-cache musl-dev openssl-dev openssl-libs-static pkgconfig lua5.4-dev lua5.4-libs

WORKDIR /app

# Cache dependencies by copying manifests first
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build release binary statically linked
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --target x86_64-unknown-linux-musl

# Strip debug symbols for smaller binary
RUN strip /app/target/x86_64-unknown-linux-musl/release/ledger

# ── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM gcr.io/distroless/static-debian12:nonroot

# Copy the compiled binary
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/ledger /usr/local/bin/ledger

# Data volume for sessions, certs, and config
VOLUME ["/data"]
ENV LEDGER_DATA_DIR=/data

# Expose the default proxy port
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/ledger"]
CMD ["capture"]
