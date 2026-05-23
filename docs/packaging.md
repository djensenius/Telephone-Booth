# Packaging & systemd

Production booths run the Rust client as a `systemd` service installed
from a `.deb` package built in CI.

## Building the `.deb`

Locally (cross-compiling to aarch64):

```sh
just deb aarch64-unknown-linux-gnu
```

Under the hood this runs:

```sh
cross build -p booth-bin --release --target <triple> --features pi
cargo deb -p booth-bin --no-build --target <triple>
```

CI builds `.deb`s for both `aarch64-unknown-linux-gnu` (Pi 4 / Pi 5 / CM4)
and `armv7-unknown-linux-gnueabihf` (Pi 3) on every `workflow_dispatch` of
the `publish` workflow.

## Installing

```sh
sudo apt install ./telephone-booth_0.1.0_arm64.deb
sudo nano /etc/phone-booth/config.toml      # paste in operator token, etc.
sudo systemctl enable --now telephone-booth.service
```

## Files installed

| Path                                       | Purpose                              |
| ------------------------------------------ | ------------------------------------ |
| `/usr/bin/telephone-booth`                 | The Rust binary                       |
| `/etc/phone-booth/config.toml`             | Editable config (see configuration.md)|
| `/etc/phone-booth/debug-token`             | Auto-generated debug Bearer token     |
| `/etc/phone-booth/debug-cert.{pem,key.pem}`| Self-signed TLS cert + key            |
| `/etc/phone-booth/debug-cert.fingerprint`  | SHA-256 fingerprint of the cert       |
| `/lib/systemd/system/telephone-booth.service` | systemd unit                       |
| `/lib/systemd/system/tailscale-serve-booth.service` | optional Tailscale unit      |
| `/var/lib/telephone-booth/recordings/`     | Local FLAC cache before upload        |

## systemd unit

```ini
[Unit]
Description=Telephone Booth phone client
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=notify
User=telephone-booth
Group=telephone-booth
ExecStart=/usr/bin/telephone-booth
Restart=on-failure
RestartSec=2
StateDirectory=telephone-booth
ConfigurationDirectory=phone-booth
# Hardening
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
RestrictNamespaces=true
# Allow GPIO + audio
SupplementaryGroups=gpio audio

[Install]
WantedBy=multi-user.target
```

## Upgrading

```sh
sudo apt install ./telephone-booth_<new>.deb
# systemd auto-restarts on the dpkg postinst hook
sudo systemctl status telephone-booth
```

`/etc/phone-booth/config.toml` is marked `conffile`, so `dpkg` will prompt
on conflict rather than overwriting your edits.

## Logs

All logs go to the system journal:

```sh
journalctl -u telephone-booth -f
journalctl -u telephone-booth --since "1 hour ago" -o json | jq .
```

## Removing

```sh
sudo systemctl disable --now telephone-booth
sudo apt purge telephone-booth         # also wipes /etc/phone-booth/
```

Recordings under `/var/lib/telephone-booth/recordings/` are **kept** by
`apt remove` and only removed by `apt purge`.
