set shell := ["bash", "-cu"]
set dotenv-load := true

# Default: list recipes
default:
    @just --list

# One-time developer setup
setup:
    mise install
    cargo fetch

# Full local check — what CI runs
check: fmt-check lint test docs-check

# Format all Rust code
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

# Clippy with workspace lints as errors
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run the full test suite via nextest (falls back to cargo test if nextest absent)
test:
    @if command -v cargo-nextest >/dev/null 2>&1; then \
        cargo nextest run --workspace --all-features; \
    else \
        cargo test --workspace --all-features; \
    fi

# Run the booth binary against the mock HAL for local development
dev:
    cargo run -p booth-bin --features mock

# Launch the interactive simulator TUI with full mock I/O (no audio/operator
# hardware needed). Drop `--mock` to drive the real cross-platform audio +
# HTTP adapters against a configured operator backend.
tui:
    cargo run -p booth-bin -- run --simulator --mock

# Run on real Pi hardware (only on a Pi)
run-pi:
    cargo run -p booth-bin --release --features pi

# Cross-build for a given target (e.g. just cross-build aarch64-unknown-linux-gnu)
cross-build target:
    cross build -p booth-bin --release --target {{target}}

# Build a Debian package via cargo-deb
deb:
    cargo deb -p booth-bin --target aarch64-unknown-linux-gnu

# Build rustdoc for every workspace crate; fails on missing docs
docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features

# Doc lint + link check (offline)
docs-check:
    markdownlint-cli2 "docs/**/*.md" "README.md"
    lychee --offline --no-progress "docs/**/*.md" "README.md"

# Generate / refresh the docs/README.md index
docs-index:
    @scripts/docs-index.sh

# Audit dependencies (cargo-deny + cargo-audit)
audit:
    cargo deny check
    cargo audit

# Show the effective config the binary would load (without running)
print-config:
    cargo run -p booth-bin -- --print-config

# Tail the systemd journal for the running booth service (on the Pi)
journal:
    journalctl -u telephone-booth -f -o cat

# Print Tailscale debug-surface status (on the Pi)
diagnose-tailscale:
    @tailscale status || echo "tailscaled not installed or not running"
    @tailscale serve status 2>/dev/null || true

# Attach to the simulator TUI running inside the tmux-based systemd service
attach:
    sudo tmux -S /run/telephone-booth/tmux.sock attach -t telephone-booth
