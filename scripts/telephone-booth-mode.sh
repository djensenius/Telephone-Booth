#!/usr/bin/env bash
# Switch the Telephone Booth between headless and simulator (tmux) mode.
#
# Usage:
#   telephone-booth-mode simulator   # switch to tmux simulator
#   telephone-booth-mode headless    # switch to stock headless service
#   telephone-booth-mode status      # show which mode is active
#
# Must be run as root (or with sudo).

set -euo pipefail

HEADLESS_UNIT="telephone-booth.service"
SIMULATOR_UNIT="telephone-booth-simulator.service"

usage() {
    echo "Usage: telephone-booth-mode {simulator|headless|status}"
    echo ""
    echo "  simulator  — switch to the tmux-attached simulator (SSH-attachable)"
    echo "  headless   — switch to the stock headless runtime"
    echo "  status     — show which mode is currently active"
    exit 1
}

require_root() {
    if [[ $EUID -ne 0 ]]; then
        echo "Error: this script must be run as root (try: sudo $0 $*)" >&2
        exit 1
    fi
}

switch_to_simulator() {
    require_root
    echo "Stopping ${HEADLESS_UNIT}..."
    systemctl disable --now "${HEADLESS_UNIT}" 2>/dev/null || true
    echo "Enabling ${SIMULATOR_UNIT}..."
    systemctl enable --now "${SIMULATOR_UNIT}"
    echo ""
    echo "Done. Attach to the simulator with:"
    echo "  sudo tmux -S /run/telephone-booth/tmux.sock attach -t telephone-booth"
}

switch_to_headless() {
    require_root
    echo "Stopping ${SIMULATOR_UNIT}..."
    systemctl disable --now "${SIMULATOR_UNIT}" 2>/dev/null || true
    echo "Enabling ${HEADLESS_UNIT}..."
    systemctl enable --now "${HEADLESS_UNIT}"
    echo ""
    echo "Done. Headless runtime is active."
}

show_status() {
    local sim_active head_active
    sim_active=$(systemctl is-active "${SIMULATOR_UNIT}" 2>/dev/null || true)
    head_active=$(systemctl is-active "${HEADLESS_UNIT}" 2>/dev/null || true)

    if [[ "$sim_active" == "active" ]]; then
        echo "Mode:   simulator (tmux)"
        echo "Attach: sudo tmux -S /run/telephone-booth/tmux.sock attach -t telephone-booth"
        echo "        (or: just attach)"
        echo "Detach: Ctrl+B, D"

        local socket="/run/telephone-booth/tmux.sock"
        local session="telephone-booth"
        if [[ -S "$socket" ]] && tmux -S "$socket" has-session -t "$session" 2>/dev/null; then
            local clients
            clients=$(tmux -S "$socket" list-clients -t "$session" 2>/dev/null | wc -l | tr -d ' ')
            if [[ "$clients" == "0" ]]; then
                echo "tmux:   session up, no clients attached"
            else
                echo "tmux:   session up, ${clients} client(s) attached"
            fi
        else
            echo "tmux:   session not running yet (service may still be starting)"
        fi
    elif [[ "$head_active" == "active" ]]; then
        echo "Mode: headless"
    else
        echo "Mode: neither service is running"
    fi
}

case "${1:-}" in
    simulator) switch_to_simulator ;;
    headless)  switch_to_headless ;;
    status)    show_status ;;
    *)         usage ;;
esac
