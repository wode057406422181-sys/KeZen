# syntax=docker/dockerfile:1

# ── Stage 1: Build kezen ──────────────────────────────────────
FROM rust:1.94-bookworm AS builder-kezen

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build
# Named context "kezen-src" -> root directory
COPY --from=kezen-src Cargo.toml Cargo.lock build.rs ./
COPY --from=kezen-src proto/ proto/
# Dependency layer cache
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release 2>/dev/null || true
# Final build
COPY --from=kezen-src src/ src/
RUN touch src/main.rs && cargo build --release

# ── Stage 2: Build mock-llm-server ───────────────────────────
FROM rust:1.94-bookworm AS builder-mock

WORKDIR /build
# Named context "mock-src" -> mock-llm-server directory
COPY --from=mock-src Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release 2>/dev/null || true
COPY --from=mock-src src/ src/
RUN touch src/main.rs && cargo build --release

# ── Stage 3: Minimal runtime ────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -s /bin/sh kezen
USER kezen
RUN mkdir -p /home/kezen/.kezen/logs \
             /home/kezen/.kezen/sessions \
             /home/kezen/.kezen/api_logs

COPY --from=builder-kezen /build/target/release/kezen /usr/local/bin/kezen
COPY --from=builder-mock  /build/target/release/mock-llm-server /usr/local/bin/mock-llm-server
