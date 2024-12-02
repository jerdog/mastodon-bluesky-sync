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
    rm -rf /var/lib/apt/lists/* && \
    mkdir -p /app/config /app/logs && \
    chown -R nobody:nogroup /app

# Copy the binary from builder
COPY --from=builder /usr/src/app/target/release/mastodon-bluesky-sync /app/

# Copy the config file
COPY mastodon-bluesky-sync.toml /app/

# Create the run script
RUN echo '#!/bin/sh\n\
# First run: skip existing posts\n\
echo "First run: skipping existing posts..."\n\
/app/mastodon-bluesky-sync --config /app/mastodon-bluesky-sync.toml --skip-existing-posts\n\
\n\
# Subsequent runs: check for new posts every 1 minute\n\
echo "Starting continuous sync (every 1 minute)..."\n\
while true; do\n\
    /app/mastodon-bluesky-sync --config /app/mastodon-bluesky-sync.toml $EXTRA_ARGS\n\
    echo "Waiting 1 minute before next sync..."\n\
    sleep 60\n\
done' > /app/run.sh && \
    chmod +x /app/run.sh && \
    chown nobody:nogroup /app/run.sh

USER nobody

ENTRYPOINT ["/app/run.sh"]
