#!/bin/bash

# Set debug environment variables
export RUST_LOG=debug,mastodon_bluesky_sync=trace,reqwest=debug,hyper=debug,bsky_sdk=debug,megalodon=debug
export RUST_BACKTRACE=1

# Create logs directory if it doesn't exist
mkdir -p logs

# Run in debug mode
echo "$(date): Running sync in debug mode..." | tee -a logs/sync.log
cargo run -- 2>&1 | tee -a logs/sync.log
