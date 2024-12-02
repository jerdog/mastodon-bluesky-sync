#!/bin/bash

# Initialize variables
DEBUG_MODE=false
CARGO_ARGS=()

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --debug)
            DEBUG_MODE=true
            shift
            ;;
        -c|--config)
            CARGO_ARGS+=("--config" "$2")
            shift 2
            ;;
        -n|--dry-run)
            CARGO_ARGS+=("--dry-run")
            shift
            ;;
        --skip-existing-posts)
            CARGO_ARGS+=("--skip-existing-posts")
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--debug] [-c|--config <file>] [-n|--dry-run] [--skip-existing-posts]"
            exit 1
            ;;
    esac
done

# Set up debug mode if requested
if [ "$DEBUG_MODE" = true ]; then
    echo "Running in debug mode..."
    export RUST_LOG=debug,mastodon_bluesky_sync=trace,reqwest=debug,hyper=debug,bsky_sdk=debug,megalodon=debug
    export RUST_BACKTRACE=1
else
    echo "Running in normal mode..."
    # Reset any existing debug variables
    unset RUST_LOG
    unset RUST_BACKTRACE
fi

# Create logs directory if it doesn't exist
mkdir -p logs

echo "$(date): Running sync..." | tee -a logs/sync.log
if [ ${#CARGO_ARGS[@]} -eq 0 ]; then
    cargo run 2>&1 | tee -a logs/sync.log
else
    cargo run -- "${CARGO_ARGS[@]}" 2>&1 | tee -a logs/sync.log
fi
