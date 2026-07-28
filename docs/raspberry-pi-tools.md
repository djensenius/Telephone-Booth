# Raspberry Pi extras — installing your preferred CLI tools

These are personal quality-of-life command-line tools for a Raspberry Pi. They
are **not** required by the Telephone Booth client — the booth service runs fine
without any of them. This page collects the best way to install the latest
version of each.

> **Assumes 64-bit Raspberry Pi OS (`arm64`)**, which is what
> [the setup guide](raspberry-pi-setup.md) provisions (Pi 4 / 5, Raspberry Pi
> OS Lite 64-bit). Several of these projects publish `aarch64` Linux binaries
> only, so on a 32-bit `armhf` install the mise steps below will fail — use
> apt there instead.

There are two install routes:

- **[mise](https://mise.jdx.dev)** — the default for these tools. One manifest
  keeps everything current, and a ready-made config lives at
  [`packaging/raspberry-pi/mise.toml`](../packaging/raspberry-pi/mise.toml).
- **apt** — for the handful of tools mise cannot install, and where you would
  rather updates ride along with `apt upgrade`.

## Summary

| Tool | Recommended install | apt alternative |
| --- | --- | --- |
| `fish` | apt — no mise package | Raspberry Pi OS (Debian) |
| `mise` | apt repo below | official mise apt repo |
| `tmux` | mise (`tmux@latest`) | Raspberry Pi OS (Debian) |
| `bat` | mise (`bat@latest`) | Raspberry Pi OS, as `batcat` |
| `eza` | mise (`eza@latest`) | Pi OS 13+, or `deb.gierens.de` |
| `neovim` | mise | Debian's build, but it is stale |
| `starship` | mise | none |
| `atuin` | mise | none |
| `zellij` | mise | none |
| `bottom` (`btm`) | mise | `.deb` on GitHub releases |
| `git-delta` | mise | `.deb` on GitHub releases |
| `herdr` | mise | none |

## Install mise first

[mise](https://mise.jdx.dev) publishes its own apt repository:

```sh
sudo apt update
sudo apt install -y gpg wget
sudo install -dm 755 /etc/apt/keyrings
wget -qO - https://mise.jdx.dev/gpg-key.pub \
  | gpg --dearmor \
  | sudo tee /etc/apt/keyrings/mise-archive-keyring.gpg 1>/dev/null
echo "deb [signed-by=/etc/apt/keyrings/mise-archive-keyring.gpg arch=arm64] https://mise.jdx.dev/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/mise.list
sudo apt update
sudo apt install -y mise
```

Then activate it in your shell (fish shown; see the mise docs for bash/zsh):

```sh
echo 'mise activate fish | source' >> ~/.config/fish/config.fish
```

## Install the tools

Copy the ready-made manifest and let mise do the rest:

```sh
mkdir -p ~/.config/mise
cp packaging/raspberry-pi/mise.toml ~/.config/mise/config.toml
mise trust ~/.config/mise/config.toml
mise install
```

That installs `tmux`, `bat`, `eza`, `neovim`, `starship`, `atuin`, `zellij`,
`bottom`, `git-delta`, and `herdr` onto your `PATH` as prebuilt `aarch64`
binaries — no compiling. `mise upgrade` later bumps them to the newest
releases.

Or install them ad hoc:

```sh
mise use -g tmux@latest bat@latest eza@latest neovim@latest starship@latest \
  atuin@latest zellij@latest bottom@latest delta@latest herdr@latest
```

> The Pi manifest is intentionally separate from the repo-root
> [`mise.toml`](../mise.toml), which pins the Rust toolchain and build tooling
> the booth itself needs.

## fish — apt only

`fish` has no mise registry entry, so install it from Raspberry Pi OS:

```sh
sudo apt update
sudo apt install -y fish
```

Make it your login shell if you want:

```sh
chsh -s "$(command -v fish)"
```

## apt alternatives

You do not need any of these if you used the mise manifest above — they are
here for when you would rather have apt manage a tool.

### tmux and bat

```sh
sudo apt install -y tmux bat
```

Debian installs the `bat` binary as `batcat` (name collision). Add a shim if
you want the upstream command name:

```sh
mkdir -p ~/.local/bin
ln -sf "$(command -v batcat)" ~/.local/bin/bat
```

### eza

Raspberry Pi OS 13 (trixie) and newer ship `eza` directly, though it lags
upstream:

```sh
sudo apt install -y eza
```

On older releases, use `deb.gierens.de` — the Debian route documented in eza's
own [`INSTALL.md`](https://github.com/eza-community/eza/blob/main/INSTALL.md),
run by an eza maintainer with its signing key served from the upstream
repository:

```sh
sudo apt update
sudo apt install -y gpg
sudo mkdir -p /etc/apt/keyrings
wget -qO- https://raw.githubusercontent.com/eza-community/eza/main/deb.asc \
  | sudo gpg --dearmor -o /etc/apt/keyrings/gierens.gpg
echo "deb [arch=arm64 signed-by=/etc/apt/keyrings/gierens.gpg] https://deb.gierens.de stable main" \
  | sudo tee /etc/apt/sources.list.d/gierens.list
sudo chmod 644 /etc/apt/keyrings/gierens.gpg /etc/apt/sources.list.d/gierens.list
sudo apt update
sudo apt install -y eza
```

### neovim

Debian's `neovim` package installs with `apt install -y neovim`, but it lags a
long way behind upstream. Note that upstream publishes no Linux `.deb` at all —
its release assets are AppImages and tarballs — so mise is the practical way to
run a current Neovim.

### bottom and git-delta

Both publish an `arm64` `.deb` on their GitHub releases pages. There is no apt
repo, so you would be updating these by hand:

```sh
# Example: bottom (replace VERSION)
wget https://github.com/ClementTsang/bottom/releases/download/VERSION/bottom_VERSION-1_arm64.deb
sudo dpkg -i bottom_VERSION-1_arm64.deb

# Example: git-delta
wget https://github.com/dandavison/delta/releases/download/VERSION/git-delta_VERSION_arm64.deb
sudo dpkg -i git-delta_VERSION_arm64.deb
```
