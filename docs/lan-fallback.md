# LAN fallback (self-signed TLS + fingerprint pinning)

If Tailscale isn't available — you're on the same Wi-Fi as the booth, the
tailnet is down, or you're debugging from a guest browser — you can reach
the debug panel directly on the LAN.

## How it works

On first boot, `booth-debug` generates a self-signed X.509 cert via
[`rcgen`](https://crates.io/crates/rcgen), writes it to disk, and pins the
SHA-256 fingerprint to `/etc/phone-booth/debug-cert.fingerprint`. The
binary then listens on `0.0.0.0:8443` (configurable) for TLS connections
that present a Bearer debug token.

You verify the cert **by fingerprint**, not by CA chain. The operator UI's
Debug tab stores the fingerprint on first successful connect and refuses
to talk to a booth whose cert has changed.

## Finding the fingerprint

On the Pi:

```sh
sudo cat /etc/phone-booth/debug-cert.fingerprint
# e.g.  3a:6d:0f:…:c4:91
```

Also printed to the systemd journal at every startup:

```sh
journalctl -u telephone-booth -g 'cert fingerprint'
```

## Pinning in the operator UI

1. Settings → Debug → **Add a booth**.
2. Paste the booth's LAN URL: `https://192.168.1.42:8443`.
3. Paste the Bearer debug token (from `/etc/phone-booth/debug-token`).
4. On first connect the UI shows the cert fingerprint and asks you to
   confirm it matches the one from the Pi. Confirm → it's stored.
5. If the booth's cert ever rotates, the UI will refuse to connect and
   ask you to re-pin.

## Direct browser access

Browsers won't trust the self-signed cert, so you'll get a warning page
the first time. After the warning, the panel works normally.

To skip the warning, **install the booth's cert** as a trusted root on the
machine doing the debugging:

```sh
scp pi@booth-1.local:/etc/phone-booth/debug-cert.pem .
# macOS:  add to Keychain → System → mark "Always trust"
# Linux:  copy into /usr/local/share/ca-certificates/, run update-ca-certificates
```

## Rotating the cert

```sh
sudo rm /etc/phone-booth/debug-cert.{pem,key}.pem /etc/phone-booth/debug-cert.fingerprint
sudo systemctl restart telephone-booth
sudo cat /etc/phone-booth/debug-cert.fingerprint   # new fingerprint
```

Then re-pin in the operator UI.

## Disabling the LAN listener

Set `debug.lan_enabled = false` in `/etc/phone-booth/config.toml` and
restart. The Tailscale-backed listener (loopback + `tailscale serve`)
keeps working.
