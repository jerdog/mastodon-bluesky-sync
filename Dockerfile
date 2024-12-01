# Build stage
FROM rust:latest AS builder

WORKDIR /usr/src/app
COPY . .

# Install build dependencies
RUN apt-get update && \
    apt-get install -y pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

# Copy the binary and config file from builder
COPY --from=builder /usr/src/app/target/release/mastodon-bluesky-sync /app/mastodon-bluesky-sync
COPY mastodon-bluesky-sync.toml /app/mastodon-bluesky-sync.toml

# Create a wrapper script to run the sync in a loop
RUN echo '#!/bin/sh\nwhile true; do\n  /app/mastodon-bluesky-sync\n  echo "Waiting 2 minutes before next sync..."\n  sleep 200\ndone' > /app/run.sh && \
    chmod +x /app/run.sh

# Set the wrapper script as the entrypoint
ENTRYPOINT ["/app/run.sh"]
