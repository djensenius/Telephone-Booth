# Raspberry Pi setup (from scratch)

This guide walks you through setting up a brand-new Raspberry Pi to run the
Telephone Booth client. Each section is collapsible — expand only the parts
you need.

By the end you will have a Pi running the `telephone-booth` service, connected
to the operator backend over Tailscale, with a rotary phone wired up and
working.

---

<details>
<summary>1. Prerequisites</summary>

You will need:

- Raspberry Pi 4 or 5 (2 GB RAM minimum; 4 GB recommended)
- microSD card (16 GB+, Class 10 / A1)
- USB-C power supply (official Pi PSU recommended)
- USB-Audio-Class 2.0 interface (Focusrite Scarlett Solo / 2i2, or any UAC2
  device)
- Rotary telephone with hook switch, pulse, and gate wires accessible
- Ethernet cable **or** Wi-Fi credentials
- A computer with [Raspberry Pi Imager](https://www.raspberrypi.com/software/)
  installed
- SSH client on your computer

</details>

<details>
<summary>2. Flash Raspberry Pi OS</summary>

1. Open **Raspberry Pi Imager**.
2. Choose **Raspberry Pi OS Lite (64-bit)** — the booth needs no desktop
   environment.
3. Select your microSD card as the target.
4. Click the **gear icon** (or press `Ctrl+Shift+X`) to open Advanced Options:
   - **Set hostname:** e.g. `booth`
   - **Enable SSH:** use password authentication or paste your public key
   - **Set username and password:** e.g. `pi` / a strong password
   - **Configure Wi-Fi** (if not using Ethernet): enter SSID + passphrase,
     select your country code
   - **Set locale:** your timezone and keyboard layout
5. Write the image and eject the card.

Insert the card into the Pi, connect Ethernet (if applicable) and power, then
wait about 60 seconds for first boot.

</details>

<details>
<summary>3. First boot and system configuration</summary>

SSH into the Pi:

```sh
ssh pi@booth.local
```

Update the system:

```sh
sudo apt update && sudo apt full-upgrade -y
sudo reboot
```

After reboot, SSH back in and install baseline packages:

```sh
sudo apt install -y git curl alsa-utils
```

## Verify USB audio

Plug in your USB audio interface, then confirm ALSA sees it:

```sh
aplay -l
arecord -l
```

You should see your interface listed (e.g. `card 1: USB [Scarlett Solo USB]`).
If it does not appear, try a different USB port or check `dmesg | tail -20`
for errors.

## Set the USB interface as default (optional)

If the Pi has an on-board audio device that claims card 0, you can pin the
USB interface as the default in `/etc/asound.conf`:

```text
defaults.pcm.card 1
defaults.ctl.card 1
```

Replace `1` with whatever card number `aplay -l` reported for your interface.

</details>

<details>
<summary>4. Install Tailscale</summary>

Tailscale provides the encrypted tunnel the booth uses to reach the operator
backend and expose the debug surface with a real TLS certificate.

### Install and authenticate

```sh
curl -fsSL https://tailscale.com/install.sh | sh
sudo tailscale up \
  --hostname=telephone-booth \
  --ssh \
  --accept-routes
```

Follow the printed URL to authorize the node in your Tailscale admin console.

**Flags explained:**

- `--hostname=telephone-booth` — sets a stable MagicDNS name
  (`telephone-booth.<tailnet>.ts.net`)
- `--ssh` — enables Tailscale SSH (no need to manage SSH keys)
- `--accept-routes` — allows using subnet routes advertised by other nodes

### Verify

```sh
tailscale status
```

should show the Pi as online. Test SSH from another tailnet device:

```sh
ssh telephone-booth
```

### Ensure it survives reboots

The Tailscale installer enables the systemd service by default, but verify:

```sh
sudo systemctl enable tailscaled
sudo systemctl status tailscaled
```

Should show `enabled` and `active (running)`. After a reboot, Tailscale will
automatically reconnect.

> **Tip:** If your tailnet uses ACLs, ensure the booth node can reach the
> operator backend host on the relevant port (typically 443). See
> [tailscale.md](tailscale.md) for ACL examples.

</details>

<details>
<summary>5. Deploy the Telephone Booth client</summary>

## Option A — APT repository (recommended)

The project publishes a signed APT repository on GitHub Pages
(see [ADR 0007](adr/0007-apt-distribution.md)). On the Pi:

```sh
curl -fsSL https://djensenius.github.io/Telephone-Booth/telephone-booth-archive-keyring.gpg \
  | sudo install -m 0644 /dev/stdin /usr/share/keyrings/telephone-booth-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/telephone-booth-archive-keyring.gpg] https://djensenius.github.io/Telephone-Booth stable main" \
  | sudo tee /etc/apt/sources.list.d/telephone-booth.list
sudo apt update
sudo apt install -y telephone-booth
```

The `postinst` script creates the `phonebooth` system user, sets up
directories, enables and starts the service. Future upgrades land via
`sudo apt upgrade telephone-booth`, or automatically when you also enable
`unattended-upgrades` (see [packaging.md](packaging.md#automatic-upgrades)).

## Option B — cross-compile from your dev machine

For testing an unreleased branch:

```sh
just cross-build aarch64-unknown-linux-gnu   # Pi 4 / 5
# or: just cross-build armv7-unknown-linux-gnueabihf   # Pi 3 / Zero 2
just deb
scp target/aarch64-unknown-linux-gnu/debian/*.deb pi@booth.local:
ssh pi@booth.local "sudo apt install -y ./telephone-booth_*_arm64.deb"
```

## Option C — download from GitHub Releases

If you want a specific tagged release without going through APT, grab the
`.deb` from the relevant [GitHub Release](https://github.com/djensenius/Telephone-Booth/releases),
`scp` it to the Pi, and run `sudo apt install -y ./telephone-booth_*_arm64.deb`.
The package's bundled APT source list still registers the repo for future
`apt upgrade` cycles.

</details>

<details>
<summary>6. Configure the booth</summary>

Edit the environment file to supply secrets:

```sh
sudo editor /etc/phone-booth/env
```

At minimum, set:

```sh
BOOTH_OPERATOR_BASE_URL=https://operator.example.com
BOOTH_OPERATOR_TOKEN=tbo_YOUR_TOKEN_HERE
BOOTH_DEBUG_TOKEN=a-strong-random-string-at-least-16-chars
RUST_LOG=info
```

**Where to get these values:**

- **`BOOTH_OPERATOR_BASE_URL`**: Your operator deployment URL (the URL where
  your operator web interface is hosted)
- **`BOOTH_OPERATOR_TOKEN`**: Generate in the operator UI by signing in,
  dialing **6** → Settings → API tokens → Create. Copy the token (shown only
  once). Format: `tbo_...`
- **`BOOTH_DEBUG_TOKEN`**: Generate yourself with
  `openssl rand -base64 24` or similar. This protects access to the debug
  panel.

See [tailscale.md](tailscale.md#environment-variables) for the complete
environment variable reference.

For non-secret settings (GPIO pins, audio device, timeouts) edit the TOML
config:

```sh
sudo cp /etc/phone-booth/config.example.toml /etc/phone-booth/config.toml
sudo editor /etc/phone-booth/config.toml
```

See [`configuration.md`](configuration.md) for the full key reference.

After editing, restart the service:

```sh
sudo systemctl restart telephone-booth.service
```

</details>

<details>
<summary>7. Wire the rotary phone</summary>

Connect the phone's three signal wires to the Pi's 40-pin header:

| Function | Default BCM pin | Physical pin |
| --- | --- | --- |
| Hook switch | BCM 17 | 11 |
| Rotary pulse | BCM 27 | 13 |
| Rotary gate | BCM 22 | 15 |

Ground goes to physical pin 9 (or any GND pin).

All inputs use the Pi's internal pull-up resistor and read active-low (contact
closed = 0). If your phone's wiring is inverted, set `gpio.invert.<role> =
true` in the TOML config.

For full wiring details and a loopback smoke-test, see
[`hardware.md`](hardware.md).

</details>

<details>
<summary>8. Set up Tailscale Serve (debug surface)</summary>

The `.deb` ships a helper script that configures `tailscale serve` to proxy
the booth's loopback debug listener (`127.0.0.1:8080`) over your tailnet with
a real Let's Encrypt certificate:

```sh
sudo /usr/share/telephone-booth/setup-tailscale-serve.sh
sudo systemctl restart telephone-booth-tailscale-serve.service
```

Verify:

```sh
tailscale serve status
```

You can now reach the debug panel at `https://telephone-booth.<tailnet-name>.ts.net/`
using the debug token you set in step 6. See [`tailscale.md`](tailscale.md)
for more detail.

</details>

<details>
<summary>9. Verify everything works</summary>

## Service status

```sh
sudo systemctl status telephone-booth.service
```

The service should be `active (running)` with recent watchdog pings in the
journal.

## Logs

```sh
sudo journalctl -u telephone-booth.service -f
```

Look for `state_machine.ready` and `operator.connected` log lines.

## Test dial

1. Pick up the handset — the log should show `HookOff`.
2. Wait for the dial tone.
3. Dial **1** — you should hear a random question play.
4. Hang up — the log should show `HookOn` and a return to `Idle`.

## Debug panel

Open `https://telephone-booth.<tailnet-name>.ts.net/` in a browser and enter the debug
token. You should see the live pin matrix, state history, and audio meters.

</details>

<details>
<summary>10. Optional — observability with vmagent</summary>

The booth exposes a Prometheus-compatible `/metrics` endpoint on loopback. To
ship metrics to VictoriaMetrics:

```sh
sudo apt install vmagent
sudo editor /etc/phone-booth/vmagent.env       # set BOOTH_VM_REMOTE_WRITE_URL
sudo install -m 0600 /dev/null /etc/phone-booth/vmagent-token
sudo editor /etc/phone-booth/vmagent-token     # paste bearer token
sudo systemctl enable --now telephone-booth-vmagent.service
```

See [`observability.md`](observability.md) for the full pipeline and Grafana
dashboard setup.

</details>

<details>
<summary>11. Troubleshooting</summary>

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| Service fails to start | Missing operator token | Check `/etc/phone-booth/env` |
| No audio devices found | USB interface not plugged in or not recognized | Run `aplay -l`; try another USB port |
| GPIO reads stuck high/low | Wiring error or wrong pin config | Double-check physical connections and `config.toml` pins |
| Tailscale serve 502 | Booth service not listening on 8080 | Restart `telephone-booth.service` |
| Dial tones but no operator connection | Network / ACL issue | Check `tailscale ping operator-host` |

For more, see [`troubleshooting.md`](troubleshooting.md) and
[`runbook.md`](runbook.md).

</details>
