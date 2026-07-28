# Raspberry Pi extras — installing your preferred CLI tools

These are personal quality-of-life command-line tools for a Raspberry Pi. They
are **not** required by the Telephone Booth client — the booth service runs fine
without any of them. This page collects the best way to install the latest
version of each on Raspberry Pi OS (Debian, `arm64` or `armhf`).

There are two install routes:

- **apt** — preferred when an official, up-to-date package or apt repository
  exists. Updates ride along with `apt upgrade`.
- **[mise](https://mise.jdx.dev)** — for tools that have no good Raspberry Pi
  apt package. A ready-made config lives at
  [`packaging/raspberry-pi/mise.toml`](../packaging/raspberry-pi/mise.toml).

Wherever an apt command uses an architecture, `$(dpkg --print-architecture)`
expands to `arm64` on 64-bit Raspberry Pi OS and `armhf` on the 32-bit build, so
you can paste the commands verbatim.

## Summary

| Tool | apt source? | Recommended install |
| --- | --- | --- |
| `fish` | Yes — Raspberry Pi OS (Debian) | mise (`fish@latest`) |
| `tmux` | Yes — Raspberry Pi OS (Debian) | mise (`tmux@latest`) |
| `bat` | Yes — Raspberry Pi OS (Debian) | mise (`bat@latest`) |
| `mise` | Yes — official apt repo | apt repo below |
| `eza` | Yes — official apt repo (`deb.gierens.de`) | apt repo below |
| `neovim` (latest) | No arm apt/`.deb` | mise |
| `starship` | No | mise |
| `atuin` | No | mise |
| `zellij` | No | mise |
| `bottom` (`btm`) | `.deb` on GitHub releases only | mise (or `.deb`) |
| `git-delta` | `.deb` on GitHub releases only | mise (or `.deb`) |
| `herdr` | No — Rust binary | mise (`ubi` backend) |

## Tools available from apt

### Latest `fish`, `tmux`, and `bat` via mise

If you want the newest releases of these three tools, install them through
mise (the Debian/Raspberry Pi OS packages can lag upstream):

```sh
mise use -g fish@latest tmux@latest bat@latest
```

### APT fallback (`fish`, `tmux`, `bat`)

If you prefer apt-managed packages, all three are available directly in
Raspberry Pi OS:

```sh
sudo apt update
sudo apt install -y fish tmux bat
```

Debian installs the `bat` binary as `batcat` (name collision). Add a shim if
you want the upstream command name:

```sh
mkdir -p ~/.local/bin
ln -sf "$(command -v batcat)" ~/.local/bin/bat
```

The Debian `neovim` package is also installable this way, but it lags a long
way behind upstream — install a current Neovim with mise instead (see below).

### mise — official apt repo

[mise](https://mise.jdx.dev) publishes an apt repository with `arm64` and
`armhf` packages:

```sh
sudo apt update
sudo apt install -y gpg wget
sudo install -dm 755 /etc/apt/keyrings
wget -qO - https://mise.jdx.dev/gpg-key.pub \
  | gpg --dearmor \
  | sudo tee /etc/apt/keyrings/mise-archive-keyring.gpg 1>/dev/null
echo "deb [signed-by=/etc/apt/keyrings/mise-archive-keyring.gpg arch=$(dpkg --print-architecture)] https://mise.jdx.dev/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/mise.list
sudo apt update
sudo apt install -y mise
```

Then activate it in your shell (fish shown; see the mise docs for bash/zsh):

```sh
echo 'mise activate fish | source' >> ~/.config/fish/config.fish
```

### eza — official apt repo (`deb.gierens.de`)

[`eza`](https://github.com/eza-community/eza) is only in very recent Debian
releases, but the maintainers run an apt repo with `arm64` and `armhf` builds:

```sh
sudo apt update
sudo apt install -y gpg
sudo mkdir -p /etc/apt/keyrings
wget -qO- https://raw.githubusercontent.com/eza-community/eza/main/deb.asc \
  | sudo gpg --dearmor -o /etc/apt/keyrings/gierens.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/gierens.gpg] http://deb.gierens.de stable main" \
  | sudo tee /etc/apt/sources.list.d/gierens.list
sudo chmod 644 /etc/apt/keyrings/gierens.gpg /etc/apt/sources.list.d/gierens.list
sudo apt update
sudo apt install -y eza
```

## Tools without a Raspberry Pi apt package

The rest have no apt repository that serves current `arm64`/`armhf` builds:

- **`neovim`** — upstream does not publish a Linux `.deb` at all; Debian's apt
  package is stale.
- **`starship`**, **`atuin`** — distributed via install scripts / binaries, no
  apt repo.
- **`zellij`** — GitHub release binaries only.
- **`bottom`** (`btm`) and **`git-delta`** — each publishes an `arm64`/`armhf`
  `.deb` on its GitHub releases page, but there is no apt repo to track updates.
- **`herdr`** — a Rust binary (an agent-oriented terminal multiplexer) with
  GitHub release binaries and no apt package.

The tidiest way to install and keep all of these current is mise (next section).

If you prefer a plain `.deb` for `bottom` or `git-delta`, grab the matching
architecture from their release pages and install it directly:

```sh
# Example: bottom (replace VERSION and pick arm64 or armhf to match your Pi)
wget https://github.com/ClementTsang/bottom/releases/download/VERSION/bottom_VERSION-1_arm64.deb
sudo dpkg -i bottom_VERSION-1_arm64.deb

# Example: git-delta
wget https://github.com/dandavison/delta/releases/download/VERSION/git-delta_VERSION_arm64.deb
sudo dpkg -i git-delta_VERSION_arm64.deb
```

## The Raspberry Pi mise config

[`packaging/raspberry-pi/mise.toml`](../packaging/raspberry-pi/mise.toml)
installs every tool above that lacks a good apt package and also pins
`fish`/`tmux`/`bat` to `latest`. Install mise first (apt repo above), then:

```sh
mkdir -p ~/.config/mise
cp packaging/raspberry-pi/mise.toml ~/.config/mise/config.toml
mise trust ~/.config/mise/config.toml
mise install
```

`mise install` installs the configured tool versions onto your `PATH`, and
`mise upgrade` later bumps them to the newest releases. `herdr` is fetched with
mise's `ubi` backend straight from its GitHub releases.

> This config is intentionally separate from the repo-root
> [`mise.toml`](../mise.toml), which pins the Rust toolchain and build tooling
> the booth itself needs.
