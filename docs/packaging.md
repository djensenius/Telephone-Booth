# Packaging & systemd

Production booths run the Rust client from a Debian package built with
`cargo deb`. The package includes the runtime unit, Tailscale serve unit,
system user definitions, tmpfiles rules, sample config, and the
Tailscale provisioning helper.

## Building the `.deb`

CI and local release builds use the Pi and systemd features:

```sh
cross build -p booth-bin --release --target <triple> --features pi,systemd --no-default-features
cargo deb -p booth-bin --no-build --target <triple>
```

The `publish` workflow builds:

- `telephone-booth_<version>_arm64.deb` for `aarch64-unknown-linux-gnu`
  (Pi 4 / Pi 5 / CM4)
- `telephone-booth_<version>_armhf.deb` for
  `armv7-unknown-linux-gnueabihf` (Pi 3)

## Dependencies

`crates/booth-bin/Cargo.toml` declares Debian dependencies on:

- `tailscale` for `tailscaled`, MagicDNS, HTTPS certificates, and
  `tailscale serve`
- `alsa-utils` for ALSA diagnostics on the USB Focusrite path

PulseAudio is intentionally not recommended; ALSA is sufficient for the
booth hardware.

## Installed layout

| Path | Purpose |
| --- | --- |
| `/usr/bin/telephone-booth` | Rust runtime and diagnostics CLI |
| `/lib/systemd/system/telephone-booth.service` | Main phone client service |
| `/lib/systemd/system/telephone-booth-tailscale-serve.service` | `tailscale serve` proxy for `127.0.0.1:8080` |
| `/usr/lib/sysusers.d/telephone-booth.conf` | Creates the `phonebooth` system user |
| `/usr/lib/tmpfiles.d/telephone-booth.conf` | Creates `/var/lib/phone-booth`, `/var/log/phone-booth`, `/etc/phone-booth` |
| `/etc/phone-booth/env` | Conffile with `BOOTH_OPERATOR_*`, `BOOTH_DEBUG_TOKEN`, `RUST_LOG` |
| `/etc/phone-booth/config.example.toml` | Reference TOML config for non-env settings |
| `/usr/share/telephone-booth/setup-tailscale-serve.sh` | Idempotent Tailscale provisioning helper |
| `/usr/share/doc/telephone-booth/README.md` | Project README |

`/etc/phone-booth/env` is marked as a Debian conffile so local token edits
survive upgrades.

## systemd services

`telephone-booth.service` runs as `phonebooth`, checks the runtime before
start, and uses `Type=notify` with a 30 second watchdog:

```sh
sudo systemctl status telephone-booth.service
sudo journalctl -u telephone-booth.service -f
```

`telephone-booth-tailscale-serve.service` is a oneshot unit that persists
Tailscale's serve config:

```sh
sudo systemctl status telephone-booth-tailscale-serve.service
tailscale serve status
```

The `postinst` maintainer script runs `systemd-sysusers`, runs
`systemd-tmpfiles --create`, enables both services, and starts them
best-effort. `prerm` stops both services before removal.

## Installing

```sh
sudo apt install ./telephone-booth_0.1.0_arm64.deb
sudo editor /etc/phone-booth/env
sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
sudo systemctl restart telephone-booth.service telephone-booth-tailscale-serve.service
```

## Upgrading

```sh
sudo apt install ./telephone-booth_<new>_arm64.deb
sudo systemctl status telephone-booth.service
telephone-booth tailscale-status
```

State under `/var/lib/phone-booth` and local environment config under
`/etc/phone-booth/env` are preserved.

## Removing

```sh
sudo systemctl disable --now telephone-booth.service telephone-booth-tailscale-serve.service
sudo apt remove telephone-booth
```

Use `apt purge telephone-booth` only when you also intend to remove local
configuration managed by dpkg.
