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

> **Architecture warning.** The mise route assumes **64-bit Raspberry Pi OS
> (`arm64`)**. Several of these projects publish no 32-bit ARM binaries at all
> — `neovim`, `tmux`, `atuin`, `zellij`, and `herdr` ship `aarch64` Linux
> assets only — so on `armhf` those `mise install` steps will fail or fetch an
> unusable binary. The `armhf` column below lists what actually works on 32-bit
> Pi OS; prefer apt there, or run the 64-bit OS.

## Summary

| Tool | apt source? | Recommended install (`arm64`) | `armhf` |
| --- | --- | --- | --- |
| `fish` | Yes — Raspberry Pi OS (Debian) | apt | apt |
| `tmux` | Yes — Raspberry Pi OS (Debian) | mise (`tmux@latest`) | apt only |
| `bat` | Yes — Raspberry Pi OS (Debian) | mise (`bat@latest`) | mise or apt |
| `mise` | Yes — official apt repo | apt repo below | apt repo below |
| `eza` | Yes — official apt repo (`deb.gierens.de`) | apt repo below | apt repo below |
| `neovim` (latest) | No arm apt/`.deb` | mise | none — apt's stale build |
| `starship` | No | mise | mise |
| `atuin` | No | mise | none |
| `zellij` | No | mise | none |
| `bottom` (`btm`) | `.deb` on GitHub releases only | mise (or `.deb`) | `.deb` |
| `git-delta` | `.deb` on GitHub releases only | mise (or `.deb`) | `.deb` |
| `herdr` | No — Rust binary | mise | none |

## Tools available from apt

### `fish`

`fish` has no mise package — install it from Raspberry Pi OS, which carries it
for both `arm64` and `armhf`:

```sh
sudo apt update
sudo apt install -y fish
```

### Latest `tmux` and `bat` via mise

If you want newer releases than Debian ships, install these two through mise
(`arm64` only for `tmux`; `bat` also has an `armhf` build):

```sh
mise use -g tmux@latest bat@latest
```

### APT fallback (`fish`, `tmux`, `bat`)

All three are available directly in Raspberry Pi OS for every Pi
architecture:

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
way behind upstream — on `arm64`, install a current Neovim with mise instead
(see below). On 32-bit `armhf` the Debian package is the only option.

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
echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/gierens.gpg] https://deb.gierens.de stable main" \
  | sudo tee /etc/apt/sources.list.d/gierens.list
sudo chmod 644 /etc/apt/keyrings/gierens.gpg /etc/apt/sources.list.d/gierens.list
sudo apt update
sudo apt install -y eza
```

## Tools without a Raspberry Pi apt package

The rest have no apt repository that serves current `arm64`/`armhf` builds:

- **`neovim`** — upstream publishes no Linux `.deb` at all: its release assets
  are AppImages and tarballs for `x86_64` and `arm64` only. Debian's apt
  package works everywhere but is stale, and it is the only option on `armhf`.
- **`starship`**, **`atuin`** — distributed via install scripts / binaries, no
  apt repo. `starship` ships an `armhf` build; `atuin` does not.
- **`zellij`** — GitHub release binaries only, `aarch64`/`x86_64` Linux.
- **`bottom`** (`btm`) and **`git-delta`** — each publishes an `arm64`/`armhf`
  `.deb` on its GitHub releases page, but there is no apt repo to track updates.
- **`herdr`** — a Rust binary (an agent multiplexer for the terminal) with
  GitHub release binaries (`aarch64` Linux and macOS) and no apt package.

The tidiest way to install and keep all of these current is mise (next section)
— on `arm64`. On 32-bit `armhf`, only `starship`, `bat`, `bottom`, and
`git-delta` from this list have usable builds; `neovim`, `atuin`, `zellij`,
and `herdr` do not ship 32-bit ARM binaries, so use the (older) Debian package
where one exists or run the 64-bit OS.

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
`tmux`/`bat` to `latest`. It targets **64-bit Raspberry Pi OS (`arm64`)** —
see the architecture warning at the top of this page before using it on a
32-bit install. Install mise first (apt repo above), then:

```sh
mkdir -p ~/.config/mise
cp packaging/raspberry-pi/mise.toml ~/.config/mise/config.toml
mise trust ~/.config/mise/config.toml
mise install
```

`mise install` installs the configured tool versions onto your `PATH`, and
`mise upgrade` later bumps them to the newest releases. Every entry is a
first-class mise registry tool — including `herdr` — so no custom backend
prefix is needed.

> This config is intentionally separate from the repo-root
> [`mise.toml`](../mise.toml), which pins the Rust toolchain and build tooling
> the booth itself needs.
