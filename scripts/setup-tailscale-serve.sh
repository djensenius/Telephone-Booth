#!/bin/sh
set -eu

EXPECTED_TARGET="http://127.0.0.1:8080"
EXPECTED_PATH="/"
HTTPS_PORT="443"

if [ "$(id -u)" -ne 0 ]; then
    echo "setup-tailscale-serve.sh must run as root; retry with sudo." >&2
    exit 1
fi

TAILSCALE_BIN="$(command -v tailscale || true)"
if [ -z "$TAILSCALE_BIN" ]; then
    echo "tailscale is not installed. Install the telephone-booth .deb dependencies first." >&2
    exit 1
fi
if ! "$TAILSCALE_BIN" status --json >/dev/null 2>&1; then
    echo "Tailscale is not authenticated yet; running 'tailscale up'."
    "$TAILSCALE_BIN" up
fi

"$TAILSCALE_BIN" serve --bg --https="$HTTPS_PORT" --set-path="$EXPECTED_PATH" "$EXPECTED_TARGET"

if [ -x /usr/bin/telephone-booth ]; then
    STATUS_OUTPUT="$(/usr/bin/telephone-booth tailscale-status)"
elif command -v telephone-booth >/dev/null 2>&1; then
    STATUS_OUTPUT="$(telephone-booth tailscale-status)"
else
    echo "telephone-booth CLI is required to verify tailscale status." >&2
    exit 1
fi

MAGICDNS_NAME="$(printf '%s\n' "$STATUS_OUTPUT" | sed -n 's/^magicdnsname: //p')"
if [ -z "$MAGICDNS_NAME" ] || [ "$MAGICDNS_NAME" = "<unknown>" ]; then
    echo "tailscale status did not report a MagicDNS name." >&2
    printf '%s\n' "$STATUS_OUTPUT" >&2
    exit 1
fi
if ! printf '%s\n' "$STATUS_OUTPUT" | grep -F "$EXPECTED_TARGET" >/dev/null 2>&1; then
    echo "tailscale serve config does not point at $EXPECTED_TARGET." >&2
    printf '%s\n' "$STATUS_OUTPUT" >&2
    exit 1
fi
FINAL_URL="https://$MAGICDNS_NAME/"

echo "Tailscale serve is configured."
echo "MagicDNS name: $MAGICDNS_NAME"
echo "Debug URL: $FINAL_URL"

echo "Verify with: curl -H 'Authorization: Bearer <debug-token>' ${FINAL_URL}healthz"
exit 0
