#!/usr/bin/env bash
# Start the Telephone Booth simulator inside a tmux session so operators can
# attach over SSH. Designed to be invoked by systemd via
# telephone-booth-simulator.service.
#
# The tmux socket is placed under /run/telephone-booth/ (owned by phonebooth)
# so attach commands don't need to guess paths.

set -euo pipefail

TMUX_SOCKET="/run/telephone-booth/tmux.sock"
SESSION_NAME="telephone-booth"

# If the session already exists (leftover from a crash), kill it first.
if tmux -S "$TMUX_SOCKET" has-session -t "$SESSION_NAME" 2>/dev/null; then
    tmux -S "$TMUX_SOCKET" kill-session -t "$SESSION_NAME"
fi

exec tmux -S "$TMUX_SOCKET" new-session -d -s "$SESSION_NAME" \
    /usr/bin/telephone-booth run --simulator
