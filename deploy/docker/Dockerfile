# ─── Build stage ──────────────────────────────────────────────────────────
FROM rust:1.95-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /build

# Copy manifests first for better layer caching
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

# Build the release binary
# CARGO_INCREMENTAL=0 avoids rmeta encoder ICE on Windows/WSL with rustc 1.95
ENV CARGO_INCREMENTAL=0
ENV CARGO_TERM_COLOR=always

RUN cargo build --release --package eneros-api

# ─── Runtime stage ────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd --create-home --shell /bin/bash eneros

# Copy the binary from builder
COPY --from=builder /build/target/release/eneros-api /usr/local/bin/eneros-api

# Copy default config
COPY eneros.toml /etc/eneros/eneros.toml

# Create data directories
RUN mkdir -p /var/lib/eneros/memory /var/lib/eneros/timeseries /var/log/eneros \
    && chown -R eneros:eneros /var/lib/eneros /var/log/eneros /etc/eneros

# Switch to non-root user
USER eneros
WORKDIR /home/eneros

# Expose API port
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

# Default config path
ENV ENEROS_CONFIG=/etc/eneros/eneros.toml

# Run the API server
ENTRYPOINT ["eneros-api"]
CMD ["run", "--config", "/etc/eneros/eneros.toml"]
